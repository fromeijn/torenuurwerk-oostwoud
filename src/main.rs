use log::*;
use rppal::gpio::{Gpio, InputPin, Level, OutputPin};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    info!("Church clock controller started!");

    let gpio = Gpio::new()?;
    let chime_lever_pin: InputPin = gpio.get(16)?.into_input_pullup();
    let (time_of_clock_tx, time_of_clock_rx) = mpsc::channel();

    monitor_time_of_clock(chime_lever_pin, time_of_clock_tx);


    loop {
        if let Ok((transition_count, first_transition_time)) = time_of_clock_rx.recv() {
            println!("Time of Clock:");
            println!("First transition at: {:?}", first_transition_time);
            println!("Number of transitions: {}", transition_count);
        }
    }
}

/// Monitors the time of the clock
/// It is using an input pin that transitions every time the clock chimes.
/// Sends the transition count (hours or half hour) and first transition time for that session through a channel.
fn monitor_time_of_clock(chime_lever_pin: InputPin, tx: mpsc::Sender<(usize, SystemTime)>) {
    thread::spawn(move || {
        let mut prev_level = chime_lever_pin.read();
        let mut transition_count = 0;
        let mut first_transition_time: Option<Instant> = None;
        let mut first_transition_system_time: Option<SystemTime> = None;

        loop {
            let current_level = chime_lever_pin.read();

            if prev_level == Level::Low && current_level == Level::High {
                transition_count += 1;
                if first_transition_time.is_none() {
                    info!("first transition detected");
                    first_transition_time = Some(Instant::now());
                    first_transition_system_time = Some(SystemTime::now());
                } else {
                    info!("{} transition detected", transition_count);
                }
            }

            prev_level = current_level;

            // Check if the monitoring interval is over
            if first_transition_time.is_some()
                && first_transition_time.unwrap().elapsed() >= Duration::from_secs(2 * 60)
            {
                // Send the data to the main thread
                let _ = tx.send((transition_count, first_transition_system_time.unwrap()));
                // Reset state for the next interval
                transition_count = 0;
                first_transition_time = None;
            }

            thread::sleep(Duration::from_millis(100)); // Polling interval
        }
    });
}

