use console::style;

pub fn success(msg: &str) -> String {
    format!("  {} {}", style("✓").green(), msg)
}

pub fn error(msg: &str) -> String {
    format!("  {} {}", style("✗").red(), msg)
}

pub fn info_line(label: &str, value: &str) -> String {
    format!("  {:<12} {}", style(label).dim(), value)
}
