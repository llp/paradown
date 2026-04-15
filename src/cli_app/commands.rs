use std::num::NonZeroU64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandTarget {
    All,
    Tasks(Vec<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Status(CommandTarget),
    Pause(CommandTarget),
    Resume(CommandTarget),
    Cancel(CommandTarget),
    SetRateLimit(Option<NonZeroU64>),
}

pub fn parse_command(input: &str) -> Result<Command, String> {
    let normalized = input.trim();
    if normalized.is_empty() {
        return Err("Empty command".into());
    }

    let lowercase = normalized.to_ascii_lowercase();
    let mut parts = lowercase.split_whitespace();
    let command = parts.next().ok_or_else(|| "Empty command".to_string())?;
    let arguments: Vec<_> = parts.collect();

    match command {
        "help" => Ok(Command::Help),
        "status" => Ok(Command::Status(parse_target(&arguments, true)?)),
        "pause" => Ok(Command::Pause(parse_target(&arguments, true)?)),
        "resume" => Ok(Command::Resume(parse_target(&arguments, true)?)),
        "cancel" => Ok(Command::Cancel(parse_target(&arguments, true)?)),
        "limit" => parse_limit_command(&arguments),
        _ => Err(format!("Unknown command: {input}. Type 'help' for usage.")),
    }
}

pub fn help_lines() -> Vec<String> {
    vec![
        "Interactive commands:".into(),
        "  help                         Show available commands".into(),
        "  status [all|id ...]          Print task summary for all tasks or selected task ids".into(),
        "  pause [all|id ...]           Pause all tasks or selected task ids".into(),
        "  resume [all|id ...]          Resume all tasks or selected task ids".into(),
        "  cancel [all|id ...]          Cancel all tasks or selected task ids".into(),
        "  limit <kbps|off>             Set global rate limit, or disable it".into(),
    ]
}

fn parse_target(arguments: &[&str], default_all: bool) -> Result<CommandTarget, String> {
    if arguments.is_empty() {
        return if default_all {
            Ok(CommandTarget::All)
        } else {
            Err("Expected at least one task id or 'all'".into())
        };
    }

    if arguments.len() == 1 && arguments[0] == "all" {
        return Ok(CommandTarget::All);
    }

    let mut ids = Vec::with_capacity(arguments.len());
    for argument in arguments {
        let id = argument
            .parse::<u32>()
            .map_err(|_| format!("Expected task ids or 'all', got '{argument}'"))?;
        ids.push(id);
    }

    if ids.is_empty() && default_all {
        Ok(CommandTarget::All)
    } else {
        Ok(CommandTarget::Tasks(ids))
    }
}

fn parse_limit_command(arguments: &[&str]) -> Result<Command, String> {
    let value = arguments
        .first()
        .ok_or_else(|| "Usage: limit <kbps|off>".to_string())?;
    let limit = match *value {
        "off" | "none" | "unlimited" | "0" => None,
        _ => NonZeroU64::new(
            value
                .parse::<u64>()
                .map_err(|_| "limit expects a positive integer or 'off'".to_string())?,
        ),
    };
    Ok(Command::SetRateLimit(limit))
}

#[cfg(test)]
mod tests {
    use super::{Command, CommandTarget, parse_command};
    use std::num::NonZeroU64;

    #[test]
    fn parses_global_pause_command_by_default() {
        assert!(matches!(
            parse_command("pause").unwrap(),
            Command::Pause(CommandTarget::All)
        ));
    }

    #[test]
    fn parses_task_target_commands() {
        assert!(matches!(
            parse_command("resume 1 2").unwrap(),
            Command::Resume(CommandTarget::Tasks(ids)) if ids == vec![1, 2]
        ));
    }

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
    fn parses_status_all_command() {
        assert!(matches!(
            parse_command("status all").unwrap(),
            Command::Status(CommandTarget::All)
        ));
    }

    #[test]
    fn rejects_unknown_command() {
        assert!(parse_command("wat").is_err());
    }
}
