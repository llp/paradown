use std::num::NonZeroU64;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub enum Command {
    Help,
    Status,
    PauseAll,
    ResumeAll,
    CancelAll,
    SetRateLimit(Option<NonZeroU64>),
}

pub struct InteractiveMode {
    pub command_tx: mpsc::Sender<Command>,
}

impl InteractiveMode {
    pub fn new() -> (Self, mpsc::Receiver<Command>) {
        let (command_tx, command_rx) = mpsc::channel(32);
        (Self { command_tx }, command_rx)
    }

    pub async fn run(&mut self) {
        use tokio::io::AsyncBufReadExt;

        print_help();

        let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let mut line = String::new();

        loop {
            match stdin.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    self.handle_command(line.trim()).await;
                    line.clear();
                }
                Err(err) => {
                    println!("Failed to read interactive command: {err}");
                    break;
                }
            }
        }
    }

    async fn handle_command(&self, raw_command: &str) {
        let normalized = raw_command.trim();
        if normalized.is_empty() {
            return;
        }

        let command = match parse_command(normalized) {
            Ok(command) => command,
            Err(err) => {
                println!("{err}");
                return;
            }
        };

        if let Err(err) = self.command_tx.send(command).await {
            println!("Failed to send interactive command: {err}");
        }
    }
}

fn parse_command(input: &str) -> Result<Command, String> {
    let lowercase = input.to_ascii_lowercase();
    let mut parts = lowercase.split_whitespace();
    let command = parts.next().ok_or_else(|| "Empty command".to_string())?;

    match command {
        "help" => Ok(Command::Help),
        "status" => Ok(Command::Status),
        "pause" => Ok(Command::PauseAll),
        "resume" => Ok(Command::ResumeAll),
        "cancel" => Ok(Command::CancelAll),
        "limit" => {
            let value = parts
                .next()
                .ok_or_else(|| "Usage: limit <kbps|off>".to_string())?;
            let limit = match value {
                "off" | "none" | "unlimited" | "0" => None,
                _ => NonZeroU64::new(
                    value
                        .parse::<u64>()
                        .map_err(|_| "limit expects a positive integer or 'off'".to_string())?,
                ),
            };
            Ok(Command::SetRateLimit(limit))
        }
        _ => Err(format!("Unknown command: {input}. Type 'help' for usage.")),
    }
}

pub fn print_help() {
    println!("Interactive commands:");
    println!("  help                 Show available commands");
    println!("  status               Print task summary and current rate limit");
    println!("  pause                Pause all active downloads");
    println!("  resume               Resume paused downloads");
    println!("  cancel               Cancel all downloads");
    println!("  limit <kbps|off>     Set global rate limit, or disable it");
}

#[cfg(test)]
mod tests {
    use super::{Command, parse_command};
    use std::num::NonZeroU64;

    #[test]
    fn parses_limit_disable_command() {
        assert!(matches!(
            parse_command("limit off").unwrap(),
            Command::SetRateLimit(None)
        ));
    }

    #[test]
    fn parses_limit_value_command() {
        assert!(matches!(
            parse_command("limit 128").unwrap(),
            Command::SetRateLimit(Some(value)) if value == NonZeroU64::new(128).unwrap()
        ));
    }

    #[test]
    fn rejects_unknown_command() {
        assert!(parse_command("wat").is_err());
    }
}
