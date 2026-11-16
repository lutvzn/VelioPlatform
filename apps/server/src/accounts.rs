//! What the website's account forms check before Auth does, in words for a
//! person rather than GoTrue's error codes.

pub const MAX_EMAIL_LENGTH: usize = 254;

pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Why an email can't be used. Deliberately loose: the confirmation email is the real check.
pub fn email_problem(email: &str) -> Option<&'static str> {
    let email = normalize_email(email);
    if email.is_empty() {
        return Some("Enter your email address.");
    }
    if email.encode_utf16().count() > MAX_EMAIL_LENGTH {
        return Some("That email address is too long.");
    }
    // /^[^\s@]+@[^\s@]+\.[^\s@]+$/
    let looks_right = !email.chars().any(char::is_whitespace)
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.contains('@')
                && domain.char_indices().any(|(at, c)| c == '.' && at > 0 && at + 1 < domain.len())
        });
    if !looks_right {
        return Some("That doesn't look like an email address.");
    }
    None
}

/// A form's text fields, as Hono's `parseBody`: a missing field reads as "".
pub trait Fields {
    fn field(&self, name: &str) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_emails() {
        assert_eq!(normalize_email("  Owner@Example.INVALID "), "owner@example.invalid");
        assert_eq!(email_problem("owner@example.invalid"), None);
        assert_eq!(email_problem(" "), Some("Enter your email address."));
        for wrong in ["owner", "owner@example", "@example.invalid", "a b@example.invalid", "a@b@c.d", "a@.d", "a@b."] {
            assert_eq!(email_problem(wrong), Some("That doesn't look like an email address."), "{wrong}");
        }
        let long = format!("{}@example.invalid", "a".repeat(250));
        assert_eq!(email_problem(&long), Some("That email address is too long."));
    }
}
