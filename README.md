# trade_logger

A Rust logging backend that routes trade events to an HTTP reporting server while
letting regular application logs continue to print to the console.

It plugs into the standard [`log`](https://docs.rs/log) facade: once installed as
the global logger, any log record targeting `"trades"` is parsed as a `TradeLog`
and shipped over HTTP (as JSON) to a configurable endpoint, while everything else
is formatted and printed to the console as usual.

## Usage

Add as a dependency (path or git, since it isn't published to crates.io):

```toml
[dependencies]
trade_logger = { path = "../trade_logger" }
log = "0.4"
```

Initialize once at startup:

```rust
use trade_logger::{RoutingLogger, LoggingConfig};

fn main() {
    RoutingLogger::init(LoggingConfig {
        http_endpoint: "http://localhost:8080/trade/".to_string(),
        console_level: log::LevelFilter::Info,
        http_level: log::LevelFilter::Info,
    }).expect("failed to init logger");

    // regular logs go to console
    log::info!("agent started");

    // trade logs go to HTTP
    trade_logger::log_trade("EUR_USD", 100.0, 1.0850, "agent-1");

    // equivalent, via the macro (target: "trades")
    trade_logger::log_trade!("EUR_USD", 100.0, 1.0850, "agent-1");
}
```

`RoutingLogger::init` can be called only once per process (it installs the global
`log` logger). It also spawns a dedicated OS thread running its own Tokio runtime
to drive HTTP delivery, so the calling application does not need to be async.

The HTTP sender POSTs each trade as JSON to `{http_endpoint}{agent_name}/`, e.g.
`http://localhost:8080/trade/agent-1/`.

## Library structure

- `src/lib.rs` — the entire crate:
  - `TradeLog` — the serializable trade record (`datetime`, `instrument`, `units`,
    `price`, `agent_name`).
  - `LoggingConfig` — endpoint URL and per-sink level filters; `Default` points at
    `http://localhost:8080/trade/`.
  - `RoutingLogger` — implements `log::Log`. Wraps an `env_logger::Logger` for the
    console sink and an `mpsc::UnboundedSender<TradeLog>` feeding a background
    Tokio task that owns a `reqwest::Client` and POSTs each trade.
  - `log_trade!` macro and `log_trade()` function — equivalent ways to emit a
    trade; both build a `TradeLog` and route it solely via
    `log::info!(target: "trades", ...)`.

## Known issues

Reliability gaps worth addressing before relying on this under real load or an
unreliable network — the happy path (reachable, responsive, unauthenticated
endpoint) works correctly today:

1. **Unbounded channel with no backpressure or shutdown.** `mpsc::unbounded_channel`
   means a slow/unreachable HTTP endpoint causes queued `TradeLog`s to grow
   without bound — a resource-exhaustion risk in a long-running process. There's
   also no way to flush or await pending sends — `flush()` only flushes the
   console sink — so trades still in the channel are silently lost if the
   process exits (the background thread is not joined anywhere).

2. **No HTTP timeout or retry, and no auth support.** `reqwest::Client::new()`
   sets no request timeout, and the sender task processes the channel strictly
   sequentially, so a single hung request blocks every trade queued behind it
   (feeding #1's unbounded growth). Failed sends are only `eprintln!`'d and the
   trade is dropped — no retry/backoff. Separately, there's no way to attach
   auth headers/tokens, so the client can't talk to a reporting server that
   requires authentication at all — a missing capability rather than a
   reliability issue.
