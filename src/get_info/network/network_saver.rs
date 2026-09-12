use crate::command_parser::{NetworkConfig, NetworkStatisticsMode, TrafficPeriod};
use crate::get_info::network::{filter_network, update_traffic_offset};
use log::{error, info, warn};
use std::fs;
use std::time::Duration;
use sysinfo::Networks;
use time::format_description::well_known::Rfc3339;
use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, Weekday};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Represents the state of network statistics saved to disk
#[derive(PartialEq, Clone, Debug)]
struct NetworkInfo {
    config: NetworkConfig,
    boot_id: String,
    /// The total transmitted bytes accumulated in the current cycle.
    cycle_total_tx: u64,
    /// The total received bytes accumulated in the current cycle.
    cycle_total_rx: u64,
    /// Unix timestamp for the next scheduled reset.
    next_reset_timestamp: i64,
    offset_tx: i64,
    offset_rx: i64,
}

impl NetworkInfo {
    pub fn encode(&self) -> String {
        let mut output = String::new();
        // Helper macro
        macro_rules! append_line {
            ($key:expr, $value:expr) => {
                output.push_str(&format!("{}={}\n", $key, $value));
            };
        }

        // Config fields
        append_line!(
            "disable_network_statistics",
            self.config.disable_network_statistics
        );
        append_line!("network_interval", self.config.network_interval);
        append_line!("network_save_path", &self.config.network_save_path);
        append_line!("traffic_period", format!("{:?}", self.config.traffic_period));
        append_line!("traffic_reset_day", &self.config.traffic_reset_day);
        append_line!(
            "network_statistics_mode",
            format!("{:?}", self.config.network_statistics_mode)
        );
        append_line!("network_duration", self.config.network_duration);
        append_line!(
            "network_interval_number",
            self.config.network_interval_number
        );

        // NetworkInfo fields
        append_line!("boot_id", &self.boot_id);
        append_line!("cycle_total_tx", self.cycle_total_tx);
        append_line!("cycle_total_rx", self.cycle_total_rx);
        append_line!("next_reset_timestamp", self.next_reset_timestamp);
        append_line!("offset_tx", self.offset_tx);
        append_line!("offset_rx", self.offset_rx);

        output
    }

    pub fn decode(input: &str) -> Result<Self, String> {
        // For NetworkConfig
        let mut disable_network_statistics = None;
        let mut network_interval = None;
        let mut network_save_path = None;
        let mut traffic_period = None;
        let mut traffic_reset_day = None;
        let mut network_statistics_mode = None;
        let mut network_duration = None;
        let mut network_interval_number = None;

        // For NetworkInfo
        let mut boot_id = None;
        let mut cycle_total_tx = None;
        let mut cycle_total_rx = None;
        let mut next_reset_timestamp = None;
        // Default to sentinel value if not found
        let mut offset_tx = i64::MIN;
        let mut offset_rx = i64::MIN;

        for line in input.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("Format error: expected key=value, got '{}'", line))?;
            let key = key.trim();
            let value = value.trim();

            let parse_err = |type_name: &str| format!("Invalid {} for key '{}'", type_name, key);

            match key {
                // Config
                "disable_network_statistics" => {
                    disable_network_statistics =
                        Some(value.parse::<bool>().map_err(|_| parse_err("bool"))?)
                }
                "network_interval" => {
                    network_interval = Some(value.parse::<u32>().map_err(|_| parse_err("u32"))?)
                }
                "network_save_path" => network_save_path = Some(value.to_string()),
                "traffic_period" => {
                    traffic_period = match value {
                        "Week" => Some(TrafficPeriod::Week),
                        "Month" => Some(TrafficPeriod::Month),
                        "Year" => Some(TrafficPeriod::Year),
                        _ => None,
                    };
                }
                "traffic_reset_day" => traffic_reset_day = Some(value.to_string()),
                "network_statistics_mode" => {
                    network_statistics_mode = match value {
                        "Natural" => Some(NetworkStatisticsMode::Natural),
                        "Fixed" => Some(NetworkStatisticsMode::Fixed),
                        _ => None,
                    };
                }
                "network_duration" => {
                    network_duration = Some(value.parse::<u32>().map_err(|_| parse_err("u32"))?)
                }
                "network_interval_number" => {
                    network_interval_number =
                        Some(value.parse::<u32>().map_err(|_| parse_err("u32"))?)
                }

                // Info
                "boot_id" => boot_id = Some(value.to_string()),
                "cycle_total_tx" => {
                    cycle_total_tx = Some(value.parse::<u64>().map_err(|_| parse_err("u64"))?)
                }
                "cycle_total_rx" => {
                    cycle_total_rx = Some(value.parse::<u64>().map_err(|_| parse_err("u64"))?)
                }
                "next_reset_timestamp" => {
                    next_reset_timestamp =
                        Some(value.parse::<i64>().map_err(|_| parse_err("i64"))?)
                }
                "offset_tx" => offset_tx = value.parse::<i64>().map_err(|_| parse_err("i64"))?,
                "offset_rx" => offset_rx = value.parse::<i64>().map_err(|_| parse_err("i64"))?,
                _ => {} // Ignore unknown keys
            }
        }

        // Assemble the struct
        Ok(NetworkInfo {
            config: NetworkConfig {
                disable_network_statistics: disable_network_statistics
                    .ok_or("Missing field: disable_network_statistics")?,
                network_interval: network_interval.ok_or("Missing field: network_interval")?,
                network_save_path: network_save_path
                    .ok_or("Missing field: network_save_path")?,
                traffic_period: traffic_period.unwrap_or(TrafficPeriod::Month),
                traffic_reset_day: traffic_reset_day.unwrap_or_else(|| "1".to_string()),
                network_statistics_mode: network_statistics_mode
                    .unwrap_or(NetworkStatisticsMode::Fixed),
                network_duration: network_duration.unwrap_or(864_000),
                network_interval_number: network_interval_number.unwrap_or(6),
            },
            boot_id: boot_id.ok_or("Missing field: boot_id")?,
            cycle_total_tx: cycle_total_tx.ok_or("Missing field: cycle_total_tx")?,
            cycle_total_rx: cycle_total_rx.ok_or("Missing field: cycle_total_rx")?,
            next_reset_timestamp: next_reset_timestamp
                .ok_or("Missing field: next_reset_timestamp")?,
            offset_tx,
            offset_rx,
        })
    }
}

/// Main entry point for the network statistics persistence thread.
pub async fn network_saver(network_config: &NetworkConfig) {
    if network_config.disable_network_statistics {
        return;
    }

    let mut networks = Networks::new_with_refreshed_list();

    loop {
        // Initialize state, handles file creation, migration, and reset logic
        let (mut file, mut network_info) =
            match initialize_network_state_and_offset(network_config, &mut networks).await {
                Ok(state) => state,
                Err(e) => {
                    error!("Failed to initialize network statistics: {}. This feature will be disabled.", e);
                    return;
                }
            };

        // The offset for the current session is now stored in the network_info struct.
        let offset_tx = network_info.offset_tx;
        let offset_rx = network_info.offset_rx;
        info!(
            "Network statistics cycle started. Next reset on: {}",
            OffsetDateTime::from_unix_timestamp(network_info.next_reset_timestamp)
                .unwrap()
                .format(&Rfc3339)
                .unwrap()
        );

        // Add a counter to accumulate memory update times
        let mut memory_update_count = 0;

        // Main loop for the current cycle
        loop {
            tokio::time::sleep(Duration::from_secs(
                network_config.network_interval as u64,
            ))
            .await;

            let now = OffsetDateTime::now_utc().unix_timestamp();
            if now >= network_info.next_reset_timestamp {
                info!("Network statistics cycle ended. Resetting...");
                break; // Break inner loop to re-initialize
            }

            networks.refresh(true);
            let (_, _, current_total_tx, current_total_rx) = filter_network(&networks);

            // Update the live total traffic value using the constant offset for this cycle
            network_info.cycle_total_tx = (current_total_tx as i64 + offset_tx).max(0) as u64;
            network_info.cycle_total_rx = (current_total_rx as i64 + offset_rx).max(0) as u64;

            memory_update_count += 1;
            if memory_update_count >= network_config.network_interval_number {
                // Save the updated state to the file
                if let Err(e) = save_network_info(&mut file, &network_info).await {
                    error!("Failed to save network statistics file: {}", e);
                    // Continue, maybe it's a temporary issue
                } else {
                    info!("Network statistics saved.");
                }
                memory_update_count = 0;
            }      
        }
    }
}

/// Handles all startup logic: reading/creating the state file, migrating old formats,
/// handling reboots vs. restarts, and calculating the initial traffic offset.
async fn initialize_network_state_and_offset(
    network_config: &NetworkConfig,
    networks: &mut Networks,
) -> Result<(File, NetworkInfo), String> {
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&network_config.network_save_path)
        .await
    {
        Ok(f) => f,
        Err(e) => return Err(format!("Failed to open network state file: {e}")),
    };

    let mut raw_data = String::new();
    file.read_to_string(&mut raw_data).await.map_err(|e| format!("Failed to read network state file: {e}"))?;

    let now = OffsetDateTime::now_utc();
    let new_boot_id = get_boot_id();

    let mut network_info = if raw_data.is_empty() {
        info!("Creating new network statistics file.");
        let next_reset_timestamp = calculate_next_reset_timestamp(network_config, now)?;
        NetworkInfo {
            config: network_config.clone(),
            boot_id: new_boot_id.clone(),
            cycle_total_tx: 0,
            cycle_total_rx: 0,
            next_reset_timestamp,
            offset_tx: i64::MIN + 2,    // initial value for tx
            offset_rx: i64::MIN + 2,
        }
    } else if let Ok(info) = NetworkInfo::decode(&raw_data) {
        info!("Loaded network statistics from file.");
        info
    } else {
        warn!("Network statistics file is corrupted or in an old format. Creating a new one.");
        let (old_tx, old_rx) =
            if let Some((tx, rx)) = parse_old_format_for_migration(&raw_data) {
                info!("Successfully migrated traffic data from old format.");
                (tx, rx)
            } else {
                (0, 0)
            };
        let next_reset_timestamp = calculate_next_reset_timestamp(network_config, now)?;
        NetworkInfo {
            config: network_config.clone(),
            boot_id: new_boot_id.clone(),
            cycle_total_tx: old_tx,
            cycle_total_rx: old_rx,
            next_reset_timestamp,
            offset_tx: i64::MIN + 2,
            offset_rx: i64::MIN + 2,
        }
    };

    // 1. Check for configuration change
    if &network_info.config != network_config {
        warn!("Network configuration changed. Resetting statistics.");
        network_info.next_reset_timestamp = calculate_next_reset_timestamp(network_config, now)?;
        network_info.config = network_config.clone();
    }

    // 2. Check if the cycle has reset since the last run
    if now.unix_timestamp() >= network_info.next_reset_timestamp {
        info!("New statistics cycle detected. Resetting totals.");
        network_info.cycle_total_tx = 0;
        network_info.cycle_total_rx = 0;
        network_info.next_reset_timestamp = calculate_next_reset_timestamp(network_config, now)?;
        network_info.offset_tx = i64::MIN;
        network_info.offset_rx = i64::MIN;
    }

    // 3. Handle reboot: if boot ID changed, invalidate the offset from the file.
    let is_reboot = cfg!(any(target_os = "linux", target_os = "android"))
        && !new_boot_id.is_empty()
        && network_info.boot_id != new_boot_id;
    if is_reboot {
        info!("System reboot detected. Invalidating saved offset.");
        network_info.offset_tx = i64::MIN + 1;
        network_info.offset_rx = i64::MIN + 1;
    }
    network_info.boot_id = new_boot_id;

    // 4. Calculate and set the initial offset for this session
    networks.refresh(true);
    let (_, _, current_total_tx, current_total_rx) = filter_network(networks);

    if network_info.offset_tx == i64::MIN {
        // Offset is invalid (due to reboot, new cycle, or new file) and must be recalculated.
        let new_offset_tx = (network_info.cycle_total_tx as i64) - (current_total_tx as i64);
        let new_offset_rx = (network_info.cycle_total_rx as i64) - (current_total_rx as i64);
        info!("Recalculated network offset: tx={}, rx={}", new_offset_tx, new_offset_rx);
        network_info.offset_tx = new_offset_tx;
        network_info.offset_rx = new_offset_rx;
    } else if network_info.offset_tx == (i64::MIN + 1) {
        network_info.offset_tx = network_info.cycle_total_tx as i64;
        network_info.offset_rx = network_info.cycle_total_rx as i64;
        info!("reboot in one statistics cycle, network offset: tx={}, rx={}", network_info.offset_tx, network_info.offset_rx);
    } else if network_info.offset_tx == (i64::MIN + 2) {
        if network_info.cycle_total_tx == 0 && network_info.cycle_total_rx == 0 { 
            network_info.offset_tx = 0;
            network_info.offset_rx = 0;
        } else {
            network_info.offset_tx = (network_info.cycle_total_tx as i64) - (current_total_tx as i64);
            network_info.offset_rx = (network_info.cycle_total_rx as i64) - (current_total_rx as i64);
        }
        info!("initial statistics cycle, network offset: tx={}, rx={}", network_info.offset_tx, network_info.offset_rx);
    } else {
        // Offset from file is valid (program restart without reboot). Use it directly.
        info!("Using existing network offset from file: tx={}, rx={}", network_info.offset_tx, network_info.offset_rx);
    }

    update_traffic_offset(network_info.offset_tx, network_info.offset_rx);

    // 5. Save the potentially updated state (new boot_id, new cycle, and new offset)
    save_network_info(&mut file, &network_info)
        .await
        .map_err(|e| format!("Failed to save initial network state: {e}"))?;

    Ok((file, network_info))
}

/// Calculates the timestamp of the next reset event based on the configuration.
fn calculate_next_reset_timestamp(
    config: &NetworkConfig,
    now: OffsetDateTime,
) -> Result<i64, String> {
    // for fixed mode
    if config.network_statistics_mode == NetworkStatisticsMode::Fixed {
        return Ok(now.unix_timestamp() + i64::from(config.network_duration));
    }

    // for natural mode
    let period = &config.traffic_period;
    let reset_day_str = &config.traffic_reset_day;

    let mut next_reset_date = now.date();

    match period {
        TrafficPeriod::Week => {
            let target_weekday = match reset_day_str.to_lowercase().as_str() {
                "mon" | "1" => Weekday::Monday,
                "tue" | "2" => Weekday::Tuesday,
                "wed" | "3" => Weekday::Wednesday,
                "thu" | "4" => Weekday::Thursday,
                "fri" | "5" => Weekday::Friday,
                "sat" | "6" => Weekday::Saturday,
                "sun" | "7" => Weekday::Sunday,
                _ => {
                    return Err(format!(
                        "Invalid weekday '{}', must be 1-7 or mon-sun",
                        reset_day_str
                    ))
                }
            };

            let mut days_to_add =
                target_weekday.number_from_monday() as i64 - now.weekday().number_from_monday() as i64;
            if days_to_add <= 0 {
                days_to_add += 7;
            }
            next_reset_date = next_reset_date + time::Duration::days(days_to_add);
        }
        TrafficPeriod::Month => {
            let reset_day = reset_day_str
                .parse::<u8>()
                .map_err(|_| format!("Invalid day of month '{}', must be a number 1-31", reset_day_str))?;
            if !(1..=31).contains(&reset_day) {
                return Err(format!("Invalid day of month '{}', must be 1-31", reset_day));
            }

            let mut target_date = now.date();
            let max_day_current_month = days_in_month(target_date.year(), target_date.month());
            let target_day = reset_day.min(max_day_current_month);

            if now.day() >= target_day {
                let (year, month) = if target_date.month() == Month::December {
                    (target_date.year() + 1, Month::January)
                } else {
                    (target_date.year(), target_date.month().next())
                };
                target_date = Date::from_calendar_date(year, month, 1).unwrap();
            }

            let max_day_target_month = days_in_month(target_date.year(), target_date.month());
            let final_day = reset_day.min(max_day_target_month);
            next_reset_date = target_date.replace_day(final_day).unwrap();
        }
        TrafficPeriod::Year => {
            let parts: Vec<&str> = reset_day_str.split('/').collect();
            if parts.len() != 2 {
                return Err(format!(
                    "Invalid date format for year reset '{}', expected 'MM/DD'",
                    reset_day_str
                ));
            }
            let month = parts[0].parse::<u8>().map_err(|_| format!("Invalid month in '{}'", reset_day_str))?;
            let day = parts[1].parse::<u8>().map_err(|_| format!("Invalid day in '{}'", reset_day_str))?;
            let month_enum = Month::try_from(month).map_err(|_| format!("Invalid month value '{}', must be 1-12", month))?;

            let mut next_year = now.year();
            let reset_date_this_year = Date::from_calendar_date(next_year, month_enum, day)
                .map_err(|e| format!("Invalid date '{}/{}': {}", month, day, e))?;

            if now.date() >= reset_date_this_year {
                next_year += 1;
            }
            next_reset_date = Date::from_calendar_date(next_year, month_enum, day).map_err(|e| {
                format!("Invalid date '{}/{}' for year {}: {}", month, day, next_year, e)
            })?;
        }
    }

    let next_reset_datetime = PrimitiveDateTime::new(next_reset_date, Time::MIDNIGHT);
    Ok(next_reset_datetime.assume_offset(now.offset()).unix_timestamp())
}

/// Helper to get the number of days in a given month and year.
fn days_in_month(year: i32, month: Month) -> u8 {
    let next_month = month.next();
    let next_year = if next_month == Month::January { year + 1 } else { year };
    let last_day = Date::from_calendar_date(next_year, next_month, 1).unwrap().previous_day().unwrap();
    last_day.day()
}

/// Saves the NetworkInfo struct to the given file as a JSON string.
async fn save_network_info(file: &mut File, info: &NetworkInfo) -> Result<(), std::io::Error> {
    let content = info.encode();
    file.rewind().await?;
    file.set_len(0).await?;
    file.write_all(content.as_bytes()).await?;
    Ok(())
}

/// Gets the boot ID from the kernel. Returns an empty string on non-Linux/Android or on error.
fn get_boot_id() -> String {
    if cfg!(any(target_os = "linux", target_os = "android")) {
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|e| {
                warn!("Failed to read boot_id: {}", e);
                String::new()
            })
    } else {
        String::new()
    }
}

/// Tries to parse the old key=value format to migrate traffic totals.
fn parse_old_format_for_migration(input: &str) -> Option<(u64, u64)> {
    let mut source_tx: Option<u64> = None;
    let mut source_rx: Option<u64> = None;

    for line in input.lines() {
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();

            match key {
                "source_tx" => source_tx = value.parse().ok(),
                "source_rx" => source_rx = value.parse().ok(),
                _ => {}
            }
        }
    }

    // We need at least source_tx and source_rx. latest_tx/rx can be 0 if not present.
    if let (Some(stx), Some(srx)) = (source_tx, source_rx) {
        Some((stx , srx))
    } else {
        None
    }
}
