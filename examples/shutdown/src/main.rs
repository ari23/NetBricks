#![feature(box_syntax)]
#![feature(asm)]
extern crate fnv;
extern crate getopts;
extern crate netbricks;
extern crate rand;
extern crate time;
use netbricks::allocators::CacheAligned;
use netbricks::common::Result;
use netbricks::config::{basic_opts, read_matches};
use netbricks::interface::*;
use netbricks::operators::{Batch, ReceiveBatch};
use netbricks::packets::{Ethernet, Packet, RawPacket};
use netbricks::scheduler::*;
use std::env;
use std::process;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const CONVERSION_FACTOR: f64 = 1000000000.;

fn test<S>(ports: Vec<CacheAligned<PortQueue>>, sched: &mut S, delay_arg: u64)
where
    S: Scheduler + Sized,
{
    for port in &ports {
        println!(
            "Receiving port {} rxq {} txq {} w/ delay {}",
            port.port.mac_address(),
            port.rxq(),
            port.txq(),
            delay_arg
        );
    }

    let pipelines: Vec<_> = ports
        .iter()
        .map(|port| {
            ReceiveBatch::new(port.clone())
                .map(move |p| delay(p, delay_arg))
                .send(port.clone())
        })
        .collect();

    println!("Running {} pipelines", pipelines.len());
    for pipeline in pipelines {
        sched.add_task(pipeline).unwrap();
    }
}

#[inline]
fn lat() {
    unsafe {
        asm!("nop"
             :
             :
             :
             : "volatile");
    }
}

#[inline]
fn delay_loop(delay: u64) {
    let mut d = 0;
    while d < delay {
        lat();
        d += 1;
    }
}

fn delay(packet: RawPacket, delay: u64) -> Result<Ethernet> {
    assert!(packet.refcnt() == 1);
    let mut ethernet = packet.parse::<Ethernet>()?;
    ethernet.swap_addresses();
    delay_loop(delay);
    Ok(ethernet)
}

fn main() {
    let mut opts = basic_opts();
    opts.optopt("d", "delay", "Delay cycles", "cycles");

    let args: Vec<String> = env::args().collect();
    let matches = match opts.parse(&args[1..]) {
        Ok(m) => m,
        Err(f) => panic!(f.to_string()),
    };
    let configuration = read_matches(&matches, &opts);

    let delay_arg = matches
        .opt_str("d")
        .unwrap_or_else(|| String::from("100"))
        .parse()
        .expect("Could not parse delay");

    match initialize_system(&configuration) {
        Ok(mut context) => {
            context.start_schedulers();

            let delay: u64 = delay_arg;
            context.add_pipeline_to_run(Arc::new(move |p, s: &mut StandaloneScheduler| {
                test(p, s, delay)
            }));
            context.execute();

            let mut pkts_so_far = (0, 0);
            let mut last_printed = 0.;
            const MAX_PRINT_INTERVAL: f64 = 30.;
            const PRINT_DELAY: f64 = 15.;
            const RUN_TIME: f64 = 60.;
            let sleep_delay = (PRINT_DELAY / 2.) as u64;
            let mut start = time::precise_time_ns() as f64 / CONVERSION_FACTOR;
            let system_boot = start;
            let sleep_time = Duration::from_millis(sleep_delay);
            println!("0 OVERALL RX 0.00 TX 0.00 CYCLE_PER_DELAY 0 0 0");
            loop {
                thread::sleep(sleep_time); // Sleep for a bit
                let now = time::precise_time_ns() as f64 / CONVERSION_FACTOR;
                if now - start > PRINT_DELAY {
                    let mut rx = 0;
                    let mut tx = 0;
                    for port in context.ports.values() {
                        for q in 0..port.rxqs() {
                            let (rp, tp) = port.stats(q);
                            rx += rp;
                            tx += tp;
                        }
                    }
                    let pkts = (rx, tx);
                    let rx_pkts = pkts.0 - pkts_so_far.0;
                    if rx_pkts > 0 || now - last_printed > MAX_PRINT_INTERVAL {
                        println!(
                            "{:.2} OVERALL RX {:.2} TX {:.2}",
                            now - start,
                            rx_pkts as f64 / (now - start),
                            (pkts.1 - pkts_so_far.1) as f64 / (now - start)
                        );
                        last_printed = now;
                        start = now;
                        pkts_so_far = pkts;
                    }
                } else if now - system_boot > RUN_TIME {
                    context.stop();
                    break;
                }
            }
        }
        Err(ref e) => {
            println!("Error: {:?}", e);
            process::exit(1);
        }
    }
}