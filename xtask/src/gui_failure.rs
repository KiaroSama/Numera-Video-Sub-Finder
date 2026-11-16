//! Bounded WebDriver request errors without request URLs.
pub fn request_error(error: reqwest::Error, route: &str) -> String {
    let detail = format!("{:?}", error.without_url());
    format!(
        "Native WebDriver request failed route={route}: {}",
        detail.chars().take(8192).collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_error_keeps_route_and_source_without_request_url() {
        let error = reqwest::Client::new().get("https://[").build().unwrap_err();
        let detail = request_error(error, "/screenshot");
        assert!(detail.contains("/screenshot") && detail.contains("source"));
        assert!(!detail.contains("https://["));
    }
}
