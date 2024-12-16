use chrono::{Local, NaiveDateTime, Timelike};
use core::fmt;
use log::{debug, info};
use rppal::gpio::{Gpio, InputPin, Level, OutputPin};
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

#[derive(Debug, PartialEq, Clone, Copy)]
enum PendulumCatcherCommand {
    Catch,
    Free,
}

impl fmt::Display for PendulumCatcherCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
enum PendulumCatcherStatus {
    Unknown,
    Error,
    Catching,
    Caught,
    Freeing,
    Freed,
}

impl fmt::Display for PendulumCatcherStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

const PENDULUM_CATCHER_COMMAND_MQTT_TOPIC: &str = "rust/PendulumCatcher/set";

#[derive(Debug, PartialEq, Clone, Copy)]
enum ClockWinderStatus {
    Unknown,
    Idle,
    WindingTimekeeping,
    WindingStriking,
}

impl fmt::Display for ClockWinderStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
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

    mqtt.subscribe(PENDULUM_CATCHER_COMMAND_MQTT_TOPIC, 0)
        .expect("Unable to subscribe to mqtt topic");
    let mqtt_rx = mqtt.start_consuming();

    let gpio = Gpio::new().expect("Unable to get raspberry pi GPIOs");
    let mut led = gpio
        .get(12)
        .expect("Unable to get LED pin")
        .into_output_high();

    let chime_lever_pin = gpio
        .get(16)
        .expect("Unable to get chime lever input")
        .into_input_pullup();
    let (time_of_clock_tx, time_of_clock_rx) = mpsc::channel();

    monitor_time_of_clock(chime_lever_pin, time_of_clock_tx);

    let pendulum_catcher_motor_enable = gpio
        .get(17)
        .expect("Unable to get pendulum motor enable pin")
        .into_output_high();
    let pendulum_catcher_motor_direction = gpio
        .get(27)
        .expect("Unable to get pendulum motor direction pin")
        .into_output_high();
    let pendulum_catcher_sense_in = gpio
        .get(21)
        .expect("Unable to get pendulum sense in pin")
        .into_input();
    let pendulum_catcher_sense_out = gpio
        .get(26)
        .expect("Unable to get pendulum sense out pin")
        .into_input();
    let (pendulum_catcher_command_tx, pendulum_catcher_command_rx) = mpsc::channel();
    let (pendulum_catcher_status_tx, pendulum_catcher_status_rx) = mpsc::channel();

    pendulum_catcher(
        pendulum_catcher_motor_enable,
        pendulum_catcher_motor_direction,
        pendulum_catcher_sense_in,
        pendulum_catcher_sense_out,
        pendulum_catcher_command_rx,
        pendulum_catcher_status_tx,
    );

    let clock_winder_motor_enable = gpio
        .get(24)
        .expect("Unable to get clock winder motor enable pin")
        .into_output_high();
    let clock_winder_motor_timekeeping_direction = gpio
        .get(22)
        .expect("Unable to get clock winder motor for timekeeping direction pin")
        .into_output_low();
    let clock_winder_motor_striking_direction = gpio
        .get(23)
        .expect("Unable to get clock winder motor for striking direction pin")
        .into_output_low();
    let clock_winder_timekeeping_request = gpio
        .get(20)
        .expect("Unable to get timekeeping winding request pin")
        .into_input();
    let clock_winder_striking_request = gpio
        .get(25)
        .expect("Unable to get striking winding request pin")
        .into_input();
    let (clock_winder_status_tx, clock_winder_status_rx) = mpsc::channel();

    clock_winder(
        clock_winder_motor_enable,
        clock_winder_motor_timekeeping_direction,
        clock_winder_motor_striking_direction,
        clock_winder_timekeeping_request,
        clock_winder_striking_request,
        clock_winder_status_tx,
    );

    let mut last_blink = Instant::now();
    let mut last_mqtt_alive = Instant::now();

    loop {
        // clock time
        if let Ok(clock_time) = time_of_clock_rx.try_recv() {
            info!("Time of Clock: {:?}", clock_time);

            mqtt.publish(paho_mqtt::Message::new(
                "rust/ClockTime",
                serde_json::to_string(&clock_time).expect("Unable to serialize clock time"),
                0,
            ))
            .expect("Unable to publish clock time to mqtt broker");
        }

        // winder status
        if let Ok(clock_winder_status) = clock_winder_status_rx.try_recv() {
            info!("Clock winder status: {:?}", clock_winder_status);

            mqtt.publish(paho_mqtt::Message::new(
                "rust/ClockWinder",
                clock_winder_status.to_string(),
                0,
            ))
            .expect("Unable to publish clock time to mqtt broker");
        }

        // pendulum catcher
        if let Ok(mqtt_message) = mqtt_rx.try_recv() {
            if let Some(mqtt_message) = mqtt_message {
                info!("received mqtt message {}", mqtt_message);
                if mqtt_message.topic() == PENDULUM_CATCHER_COMMAND_MQTT_TOPIC {
                    if mqtt_message.payload_str() == PendulumCatcherCommand::Catch.to_string() {
                        pendulum_catcher_command_tx
                            .send(PendulumCatcherCommand::Catch)
                            .unwrap()
                    } else if mqtt_message.payload_str() == PendulumCatcherCommand::Free.to_string()
                    {
                        pendulum_catcher_command_tx
                            .send(PendulumCatcherCommand::Free)
                            .unwrap()
                    }
                }
            };
        }

        if let Ok(pendulum_catcher_status) = pendulum_catcher_status_rx.try_recv() {
            info!("Pendulum catcher status: {:?}", pendulum_catcher_status);

            mqtt.publish(paho_mqtt::Message::new(
                "rust/PendulumCatcher",
                pendulum_catcher_status.to_string(),
                0,
            ))
            .expect("Unable to publish clock time to mqtt broker");
        }

        if last_mqtt_alive.elapsed() >= Duration::from_secs(10) {
            let app_status = MqttAppStatus {
                uptime_seconds: app_start_time.elapsed().as_secs(),
            };

            mqtt.publish(paho_mqtt::Message::new(
                "rust/AppStatus",
                serde_json::to_string(&app_status).expect("Unable to serialize app status"),
                0,
            ))
            .expect("Unable to publish app status to mqtt broker");
            last_mqtt_alive = Instant::now();
        }

        if last_blink.elapsed() >= Duration::from_millis(500) {
            led.toggle();
            last_blink = Instant::now();
        }

        thread::sleep(Duration::from_millis(100)); // Push data very 100 ms
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

fn clock_winder(
    mut motor_enable: OutputPin,
    mut motor_timekeeping: OutputPin,
    mut motor_striking: OutputPin,
    request_timekeeping: InputPin,
    request_striking: InputPin,
    status: mpsc::Sender<ClockWinderStatus>,
) {
    thread::spawn(move || {
        let mut last_status = ClockWinderStatus::Unknown;
        let mut current_status;
        loop {
            debug!(
                "striking {}, timekeeping {}",
                request_striking.is_low(),
                request_timekeeping.is_low()
            );
            if request_striking.is_low() {
                current_status = ClockWinderStatus::WindingStriking;
                motor_enable.set_low();
                motor_striking.set_high();
                motor_timekeeping.set_low();
            } else if request_timekeeping.is_low() {
                current_status = ClockWinderStatus::WindingTimekeeping;
                motor_enable.set_low();
                motor_striking.set_low();
                motor_timekeeping.set_high();
            } else {
                current_status = ClockWinderStatus::Idle;
                motor_enable.set_high();
                motor_striking.set_low();
                motor_timekeeping.set_low();
            }

            if current_status != last_status {
                last_status = current_status;
                status.send(current_status.clone()).unwrap();
            }
            thread::sleep(Duration::from_millis(100)); // Polling interval
        }
    });
}

fn pendulum_catcher(
    mut motor_enable: OutputPin,
    mut motor_direction: OutputPin,
    sense_in: InputPin,
    sense_out: InputPin,
    commands: mpsc::Receiver<PendulumCatcherCommand>,
    status: mpsc::Sender<PendulumCatcherStatus>,
) {
    thread::spawn(move || {
        let mut last_status = PendulumCatcherStatus::Unknown;
        let mut current_status;
        let mut start_command = Instant::now();

        motor_direction.set_high(); // inactive
        motor_enable.set_high(); // inactive

        loop {
            debug!(
                "sense in {}, sense out {}",
                sense_in.is_low(),
                sense_out.is_low()
            );

            match last_status {
                PendulumCatcherStatus::Error => {
                    motor_enable.set_high();
                    motor_direction.set_high();
                    current_status = last_status;
                }
                PendulumCatcherStatus::Catching => {
                    if sense_out.is_low() {
                        motor_enable.set_high();
                        current_status = PendulumCatcherStatus::Caught;
                    } else if start_command.elapsed() >= Duration::from_secs(2) {
                        motor_enable.set_high();
                        current_status = PendulumCatcherStatus::Error;
                    } else {
                        current_status = last_status;
                    }
                }
                PendulumCatcherStatus::Freeing => {
                    if sense_in.is_low() {
                        motor_enable.set_high();
                        current_status = PendulumCatcherStatus::Freed;
                    } else if start_command.elapsed() >= Duration::from_secs(2) {
                        motor_enable.set_high();
                        current_status = PendulumCatcherStatus::Error;
                    } else {
                        current_status = last_status;
                    }
                }
                PendulumCatcherStatus::Freed
                | PendulumCatcherStatus::Caught
                | PendulumCatcherStatus::Unknown => {
                    if sense_in.is_low() && sense_out.is_high() {
                        current_status = PendulumCatcherStatus::Freed;
                    } else if sense_in.is_high() && sense_out.is_low() {
                        current_status = PendulumCatcherStatus::Caught;
                    } else {
                        current_status = PendulumCatcherStatus::Unknown;
                    }
                }
            }

            if let Ok(command) = commands.try_recv() {
                match command {
                    PendulumCatcherCommand::Catch => {
                        motor_direction.set_low();
                        // motor_enable.set_low(); // not tested yet
                        start_command = Instant::now();
                        current_status = PendulumCatcherStatus::Catching;
                        info!("start catching pendulum");
                    }
                    PendulumCatcherCommand::Free => {
                        motor_direction.set_high();
                        // motor_enable.set_low(); // not tested yet
                        start_command = Instant::now();
                        current_status = PendulumCatcherStatus::Freeing;
                        info!("start freeing pendulum");
                    }
                }
            }

            if current_status != last_status {
                last_status = current_status;
                status.send(current_status.clone()).unwrap();
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
