use metrics::counter;

pub fn count_http_request(method: String, path: String, status: String) {
    let labels = [("method", method), ("path", path), ("status", status)];
    counter!("http_requests_total", &labels).increment(1);
}
