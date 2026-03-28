use chrono::{DateTime, Utc};

/// Generate a memory ID: YYYYMMDD-slugified-title
pub fn generate_id(title: &str, now: DateTime<Utc>) -> String {
    let date = now.format("%Y%m%d");
    let slugified = slug::slugify(title);
    format!("{date}-{slugified}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn basic_id() {
        let now = Utc.with_ymd_and_hms(2026, 3, 28, 14, 30, 0).unwrap();
        assert_eq!(
            generate_id("Deploy New App", now),
            "20260328-deploy-new-app"
        );
    }

    #[test]
    fn special_characters() {
        let now = Utc.with_ymd_and_hms(2026, 3, 28, 0, 0, 0).unwrap();
        let id = generate_id("Why gRPC > REST (for maestro)", now);
        assert!(id.starts_with("20260328-"));
        assert!(id.contains("grpc"));
        assert!(id.contains("rest"));
    }

    #[test]
    fn empty_title() {
        let now = Utc.with_ymd_and_hms(2026, 3, 28, 0, 0, 0).unwrap();
        let id = generate_id("", now);
        assert!(id.starts_with("20260328"));
    }
}
