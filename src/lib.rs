
// lib.rs - Complete logging solution with routing
use log::{Log, Metadata, Record, Level, LevelFilter, SetLoggerError};
use serde::{Serialize, Deserialize};
use std::sync::Arc;
use tokio::sync::mpsc;
use env_logger::Builder as EnvBuilder;
use std::io::Write;

/// Trade-specific log entry
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TradeLog {
    pub datetime: String,
    pub instrument: String,
    pub units: f64,
    pub price : f64,
    pub agent_name: String,
}

/// Configuration for the routing logger
#[derive(Clone)]
pub struct LoggingConfig {
    pub http_endpoint: String,
    pub console_level: LevelFilter,
    pub http_level: LevelFilter,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            http_endpoint: "http://localhost:8080/trade/".to_string(),
            console_level: LevelFilter::Info,
            http_level: LevelFilter::Info,
        }
    }
}

/// Main logger that routes logs based on target
pub struct RoutingLogger {
    console_logger: env_logger::Logger,
    http_sender: mpsc::UnboundedSender<TradeLog>,
    http_level: LevelFilter,
}

impl RoutingLogger {
    pub fn init(config: LoggingConfig) -> Result<(), SetLoggerError> {
        // Build console logger
        let console_logger = EnvBuilder::new()
            .filter_level(config.console_level)
            //.filter_module("trades", LevelFilter::Off)  // Don't show trades on console
            .format(|buf, record| {
                writeln!(
                    buf,
                    "{} [{}] {} - {}",
                    chrono::Local::now().format("%H:%M:%S%.3f"),
                    record.level(),
                    record.target(),
                    record.args()
                )
            })
            .build();

        // Setup HTTP sender
        let (tx, rx) = mpsc::unbounded_channel();
        let endpoint = config.http_endpoint.clone();
        
        // Spawn background HTTP sender
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                Self::http_sender_task(rx, endpoint).await;
            });
        });

        let logger = Self {
            console_logger,
            http_sender: tx,
            http_level: config.http_level,
        };

        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(LevelFilter::Trace);
        Ok(())
    }

    async fn http_sender_task(
        mut rx: mpsc::UnboundedReceiver<TradeLog>,
        endpoint: String,
    ) {
        let client = reqwest::Client::new();

        while let Some(trade) = rx.recv().await {
            // Build URL with agent_name as path parameter
            let url = format!("{}{}/", endpoint, trade.agent_name);
            println!("{}", url);

            match client.post(&url)
                .json(&trade)
                .send()
                .await
            {
                Ok(response) => {
                    if !response.status().is_success() {
                        eprintln!("HTTP log failed: {}", response.status());
                    }
                }
                Err(e) => eprintln!("Failed to send trade log: {}", e),
            }
        }
    }

    fn is_trade_log(&self, record: &Record) -> bool {
        // Check if target starts with "trades"
        if record.target().starts_with("trades") {
            return true;
        }

        // Also check if message is valid TradeLog JSON
        let message = format!("{}", record.args());
        if let Ok(parsed) = serde_json::from_str::<TradeLog>(&message) {
            return true;
        }

        false
    }
}

impl Log for RoutingLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        if metadata.target().starts_with("trades") {
            metadata.level() <= self.http_level
        } else {
            self.console_logger.enabled(metadata)
        }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        if self.is_trade_log(record) {
            // Parse and send to HTTP
            let message = format!("{}", record.args());
            if let Ok(trade) = serde_json::from_str::<TradeLog>(&message) {
                let _ = self.http_sender.send(trade);
            }
        } else {
            // Send to console
            self.console_logger.log(record);
        }
    }

    fn flush(&self) {
        self.console_logger.flush();
    }
}

/// Helper macro for logging trades
#[macro_export]
macro_rules! log_trade {
    ($instrument:expr, $units:expr, $price:expr, $agent:expr) => {
        log::info!(
            target: "trades",
            "{}",
            serde_json::json!({
                "datetime": chrono::Utc::now().to_rfc3339(),
                "instrument": $instrument,
                "units": $units,
                "price": $price,
                "agent_name": $agent
            })
        )
    };
}

/// Helper function for logging trades
pub fn log_trade(instrument: &str, units: f64, price: f64, agent_name: &str) {
    let trade = TradeLog {
        datetime: chrono::Utc::now().to_rfc3339(),
        instrument: instrument.to_string(),
        units,
        price,
        agent_name: agent_name.to_string(),
    };
    println!("{}", serde_json::to_string(&trade).unwrap());
    
    log::info!(
        target: "trades",
        "{}",
        serde_json::to_string(&trade).unwrap()
    );
}