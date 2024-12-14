use chrono::{Local, NaiveDateTime, Timelike};
use log::*;
use rppal::gpio::{Gpio, InputPin, Level};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Deserialize, Debug)]
struct Config {
    mqtt_host: String,
    mqtt_port: u16,
    mqtt_user: String,
    mqtt_password: String,
}

#[derive(Serialize, Debug)]
struct MqttAppStatus {
    uptime_seconds: u64,
}

#[derive(Serialize, Debug)]
struct MqttClockTime {
    number_of_chimes: u8,
    offset_seconds: f32,
}

fn read_config<P: AsRef<Path>>(path: P) -> Config {
    let config_content = std::fs::read_to_string(path).expect("Unable to read config.json");
    let config: Config =
        serde_json::from_str(&config_content).expect("Unable to parse config.json");
    config
}

fn main() {
    let app_start_time = Instant::now();
    env_logger::init();
    info!("Church clock controller started!");

    let config = read_config("./config.json");
    info!("Config: {:?}", config);

    let mqtt_server_uri =
        "mqtt://".to_string() + &config.mqtt_host + ":" + &config.mqtt_port.to_string();
    let mqtt = paho_mqtt::Client::new(mqtt_server_uri).unwrap_or_else(|err| {
        panic!("Error creating the client: {:?}", err);
    });

    let conn_opts = paho_mqtt::ConnectOptionsBuilder::new()
        .keep_alive_interval(Duration::from_secs(20))
        .clean_session(true)
        .user_name(&config.mqtt_user)
        .password(&config.mqtt_password)
        .finalize();

    if let Err(e) = mqtt.connect(conn_opts) {
        panic!("Unable to connect:\n\t{:?}", e);
    }

    let app_status = MqttAppStatus {
        uptime_seconds: app_start_time.elapsed().as_secs(),
    };

    mqtt.publish(paho_mqtt::Message::new(
        "rust/AppStatus",
        serde_json::to_string(&app_status).expect("Unable to serialize app status"),
        0,
    ))
    .expect("Unable to publish app status to mqtt broker");

    let gpio = Gpio::new().expect("Unable to get raspberry pi GPIOs");
    let chime_lever_pin: InputPin = gpio
        .get(16)
        .expect("Unable to get chime lever input")
        .into_input_pullup();
    let (time_of_clock_tx, time_of_clock_rx) = mpsc::channel();

    monitor_time_of_clock(chime_lever_pin, time_of_clock_tx);


    loop {
        if let Ok(clock_time) = time_of_clock_rx.recv() {
            info!("Time of Clock: {:?}", clock_time);

            mqtt.publish(paho_mqtt::Message::new(
                "rust/ClockTime",
                serde_json::to_string(&clock_time).expect("Unable to serialize clock time"),
                0,
            ))
            .expect("Unable to publish clock time to mqtt broker");
        }
    }
}

/// Monitors the time of the clock
/// It is using an input pin that transitions every time the clock chimes.
/// Sends the transition count (hours or half hour) and first transition time for that session through a channel.
fn monitor_time_of_clock(chime_lever_pin: InputPin, tx: mpsc::Sender<MqttClockTime>) {
    thread::spawn(move || {
        let mut prev_level = chime_lever_pin.read();
        let mut transition_count = 0;
        let mut first_transition_time: Option<Instant> = None;
        let mut first_transition_system_time: Option<NaiveDateTime> = None;

        loop {
            let current_level = chime_lever_pin.read();

            if prev_level == Level::Low && current_level == Level::High {
                transition_count += 1;
                if first_transition_time.is_none() {
                    info!("first transition detected");
                    first_transition_time = Some(Instant::now());
                    first_transition_system_time = Some(Local::now().naive_local());
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
                let clock_time = MqttClockTime {
                    number_of_chimes: transition_count,
                    offset_seconds: offset_from_half_hour(first_transition_system_time.unwrap()),
                };

                let _ = tx.send(clock_time);
                // Reset state for the next interval
                transition_count = 0;
                first_transition_time = None;
            }

            thread::sleep(Duration::from_millis(100)); // Polling interval
        }
    });
}

fn offset_from_half_hour(datetime: NaiveDateTime) -> f32 {
    let minutes = datetime.minute();
    let seconds = datetime.second();
    let total_seconds = (minutes * 60 + seconds) as f32;

    if total_seconds < 900.0 {
        // after full hour
        total_seconds
    } else if total_seconds < 1800.0 {
        // before half hour
        total_seconds - 1800.0
    } else if total_seconds < 2100.0 {
        // after half hour
        total_seconds - 1800.0
    } else {
        // before full hour
        total_seconds - 3600.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    #[test]
    fn test_exact_hour() {
        let datetime = NaiveDate::from_ymd_opt(2024, 12, 14)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        assert_eq!(offset_from_half_hour(datetime), 0.0);
    }

    #[test]
    fn test_exact_half_hour() {
        let datetime = NaiveDate::from_ymd_opt(2024, 12, 14)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap();
        assert_eq!(offset_from_half_hour(datetime), 0.0);
    }

    #[test]
    fn test_ten_past_hour() {
        let datetime = NaiveDate::from_ymd_opt(2024, 12, 14)
            .unwrap()
            .and_hms_opt(12, 10, 0)
            .unwrap();
        assert_eq!(offset_from_half_hour(datetime), 600.0);
    }

    #[test]
    fn test_ten_to_hour() {
        let datetime = NaiveDate::from_ymd_opt(2024, 12, 14)
            .unwrap()
            .and_hms_opt(11, 50, 0)
            .unwrap();
        assert_eq!(offset_from_half_hour(datetime), -600.0);
    }

    #[test]
    fn test_few_seconds_past_hour_offset() {
        let datetime = NaiveDate::from_ymd_opt(2024, 12, 14)
            .unwrap()
            .and_hms_opt(12, 0, 15)
            .unwrap();
        assert_eq!(offset_from_half_hour(datetime), 15.0);
    }

    #[test]
    fn test_few_seconds_before_hour_offset() {
        let datetime = NaiveDate::from_ymd_opt(2024, 12, 14)
            .unwrap()
            .and_hms_opt(12, 59, 45)
            .unwrap();
        assert_eq!(offset_from_half_hour(datetime), -15.0);
    }

    #[test]
    fn test_few_seconds_past_half_hour_offset() {
        let datetime = NaiveDate::from_ymd_opt(2024, 12, 14)
            .unwrap()
            .and_hms_opt(12, 30, 15)
            .unwrap();
        assert_eq!(offset_from_half_hour(datetime), 15.0);
    }

    #[test]
    fn test_few_seconds_before_half_hour_offset() {
        let datetime = NaiveDate::from_ymd_opt(2024, 12, 14)
            .unwrap()
            .and_hms_opt(12, 29, 45)
            .unwrap();
        assert_eq!(offset_from_half_hour(datetime), -15.0);
    }
}
