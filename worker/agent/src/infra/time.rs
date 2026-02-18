/// Current Unix timestamp in seconds.
pub fn now_unix_secs() -> i64 {
    chrono::Utc::now().timestamp()
}
