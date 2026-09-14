//! Routes `log` records targeting `"trades"` to an HTTP reporting endpoint,
//! and everything else to the console.
use log::{Log, Metadata, Record, LevelFilter, SetLoggerError};
use serde::{Serialize, Deserialize};
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
        record.target().starts_with("trades")
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
            match serde_json::from_str::<TradeLog>(&message) {
                Ok(trade) => {
                    let _ = self.http_sender.send(trade);
                }
                Err(e) => eprintln!("trade_logger: dropping malformed trade log record: {}", e),
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

/// Re-exported so `log_trade!` callers don't need their own `serde_json`/`chrono`/`log` deps.
pub use chrono;
pub use log;
pub use serde_json;

/// Helper macro for logging trades
#[macro_export]
macro_rules! log_trade {
    ($instrument:expr, $units:expr, $price:expr, $agent:expr) => {
        $crate::log::info!(
            target: "trades",
            "{}",
            $crate::serde_json::json!({
                "datetime": $crate::chrono::Utc::now().to_rfc3339(),
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

    log::info!(
        target: "trades",
        "{}",
        serde_json::to_string(&trade).unwrap()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Level;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sample_trade() -> TradeLog {
        TradeLog {
            datetime: "2026-09-09T12:00:00Z".to_string(),
            instrument: "EUR_USD".to_string(),
            units: 100.0,
            price: 1.0850,
            agent_name: "agent-1".to_string(),
        }
    }

    fn test_logger() -> RoutingLogger {
        let (tx, _rx) = mpsc::unbounded_channel();
        RoutingLogger {
            console_logger: EnvBuilder::new().build(),
            http_sender: tx,
            http_level: LevelFilter::Info,
        }
    }

    #[test]
    fn trade_log_json_roundtrip() {
        let trade = sample_trade();
        let json = serde_json::to_string(&trade).unwrap();
        let parsed: TradeLog = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.datetime, trade.datetime);
        assert_eq!(parsed.instrument, trade.instrument);
        assert_eq!(parsed.units, trade.units);
        assert_eq!(parsed.price, trade.price);
        assert_eq!(parsed.agent_name, trade.agent_name);
    }

    #[test]
    fn logging_config_default_points_at_localhost() {
        let config = LoggingConfig::default();
        assert_eq!(config.http_endpoint, "http://localhost:8080/trade/");
        assert_eq!(config.console_level, LevelFilter::Info);
        assert_eq!(config.http_level, LevelFilter::Info);
    }

    #[test]
    fn is_trade_log_true_for_trades_target() {
        let logger = test_logger();
        let record = Record::builder()
            .target("trades")
            .level(Level::Info)
            .args(format_args!("{{}}"))
            .build();
        assert!(logger.is_trade_log(&record));
    }

    #[test]
    fn is_trade_log_true_for_trades_subtarget() {
        let logger = test_logger();
        let record = Record::builder()
            .target("trades::agent-1")
            .level(Level::Info)
            .args(format_args!("{{}}"))
            .build();
        assert!(logger.is_trade_log(&record));
    }

    #[test]
    fn is_trade_log_does_not_content_sniff() {
        // Routing must be decided by target alone - a message that happens to
        // look like TradeLog JSON must not get rerouted off an unrelated target.
        let logger = test_logger();
        let record = Record::builder()
            .target("some_other_module")
            .level(Level::Info)
            .args(format_args!("not routed by content"))
            .build();
        assert!(!logger.is_trade_log(&record));
    }

    #[tokio::test]
    async fn http_sender_task_posts_trade_as_json() {
        let server = MockServer::start().await;
        let trade = sample_trade();

        Mock::given(method("POST"))
            .and(path("/trade/agent-1/"))
            .and(body_json(serde_json::json!({
                "datetime": trade.datetime,
                "instrument": trade.instrument,
                "units": trade.units,
                "price": trade.price,
                "agent_name": trade.agent_name,
            })))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;

        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(trade).unwrap();
        drop(tx); // closes the channel so the task's loop returns once drained

        let endpoint = format!("{}/trade/", server.uri());
        RoutingLogger::http_sender_task(rx, endpoint).await;

        // MockServer checks `.expect(1)` on drop; this makes the assertion explicit.
        server.verify().await;
    }
}