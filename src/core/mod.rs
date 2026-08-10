/// Describes whether provider usage fields are inclusive breakdowns or
/// independent counters. A provider-reported total always wins; this policy is
/// used only when a source omits its authoritative total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSemantics {
    /// OpenAI input includes cached input and output includes reasoning.
    OpenAi,
    /// Anthropic cache counters are additional input, while output includes
    /// thinking/reasoning tokens.
    Anthropic,
    /// Every component is an independent counter, as exposed by OpenCode, Pi,
    /// and Copilot's local usage database.
    Additive,
}

impl TokenSemantics {
    pub fn total(
        self,
        input: i64,
        output: i64,
        reasoning: i64,
        cache_read: i64,
        cache_write: i64,
    ) -> i64 {
        match self {
            Self::OpenAi => input + output,
            Self::Anthropic => input + output + cache_read + cache_write,
            Self::Additive => input + output + reasoning + cache_read + cache_write,
        }
    }

    pub fn cache_hit_rate(self, input: i64, cache_read: i64, cache_write: i64) -> Option<f64> {
        let denominator = match self {
            Self::OpenAi => input,
            Self::Anthropic | Self::Additive => input + cache_read + cache_write,
        };
        (denominator > 0 && cache_read > 0).then(|| cache_read as f64 / denominator as f64 * 100.0)
    }
}

pub fn token_semantics_for_agent(agent: &str) -> TokenSemantics {
    match agent {
        "codex" => TokenSemantics::OpenAi,
        "claude_code" => TokenSemantics::Anthropic,
        _ => TokenSemantics::Additive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inclusive_breakdowns_are_not_double_counted() {
        assert_eq!(TokenSemantics::OpenAi.total(100, 20, 8, 60, 0), 120);
        assert_eq!(TokenSemantics::Anthropic.total(40, 20, 8, 50, 10), 120);
        assert_eq!(TokenSemantics::Additive.total(40, 20, 8, 50, 10), 128);
    }

    #[test]
    fn cache_rate_respects_provider_input_semantics() {
        assert_eq!(
            TokenSemantics::OpenAi.cache_hit_rate(100, 60, 0),
            Some(60.0)
        );
        assert_eq!(
            TokenSemantics::Anthropic.cache_hit_rate(40, 50, 10),
            Some(50.0)
        );
    }
}
