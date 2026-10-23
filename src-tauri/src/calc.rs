//! The calculator in Ctrl+K: `=` and an expression with units, worked out by fend (fend-core: pure Rust,
//! no dependencies), as you type: `= 3 ft to cm`, `= 1 GiB / (3 MB/s) to s`, `= 0x1F + 1`. An answer
//! that takes over half a second is given up on. Currencies say they aren't available: they'd need
//! exchange rates fetched from the internet, which Glint doesn't do for this.

use std::time::{Duration, Instant};

/// What an answer may be: longer ones are cut there.
const MAX_ANSWER: usize = 1000;

struct Deadline(Instant);

impl fend_core::Interrupt for Deadline {
    fn should_interrupt(&self) -> bool {
        self.0.elapsed() > Duration::from_millis(500)
    }
}

/// An expression's value, as fend says it ("91.44 cm", "approx. 4.4428829382").
pub fn answer(expression: &str) -> Result<String, String> {
    let mut context = fend_core::Context::new();
    let result = fend_core::evaluate_with_interrupt(expression, &mut context, &Deadline(Instant::now()))?;
    let text = result.get_main_result();
    Ok(match text.char_indices().nth(MAX_ANSWER) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    })
}

#[tauri::command]
pub async fn calculate(expression: String) -> Result<String, String> {
    crate::fsx::blocking(move || answer(&expression)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn works_out_units_and_numbers() {
        assert_eq!(answer("3 ft to cm").unwrap(), "91.44 cm");
        assert_eq!(answer("5 kg * 9.81 m/s^2 to N").unwrap(), "49.05 N");
        assert_eq!(answer("0x1F + 1").unwrap(), "0x20");
        assert!(answer("1 GiB / (3 MB/s) to s").unwrap().starts_with("approx. 357.91"));
        assert!(answer("1/0").is_err());
        assert!(answer("100 USD to EUR").is_err(), "no exchange rates fetched");
        assert!(answer("10^10^10^10").is_err() || answer("10^10^10^10").unwrap().chars().count() <= MAX_ANSWER + 1, "given up on, or cut");
    }
}
