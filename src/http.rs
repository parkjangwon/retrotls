use http::header::{HeaderName, HeaderValue, HOST};
use http::{HeaderMap, Uri};

pub const HOP_BY_HOP_HEADERS: [&str; 7] = [
    "connection",
    "proxy-connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

pub fn is_hop_by_hop_header(name: &str) -> bool {
    HOP_BY_HOP_HEADERS
        .iter()
        .any(|hop_header| name.eq_ignore_ascii_case(hop_header))
}

pub fn filter_hop_by_hop_headers(headers: &mut HeaderMap) {
    for header in HOP_BY_HOP_HEADERS {
        headers.remove(header);
    }
}

pub fn set_host_header(headers: &mut HeaderMap, host: &str) {
    if let Ok(host_value) = HeaderValue::from_str(host) {
        headers.insert(HOST, host_value);
    }
}

pub fn forward_request_headers(src: &HeaderMap, dst: &mut HeaderMap, upstream_host: &str) {
    for (name, value) in src {
        if !is_hop_by_hop_header(name.as_str()) {
            dst.append(name.clone(), value.clone());
        }
    }

    set_host_header(dst, upstream_host);

    let x_forwarded_for = HeaderName::from_static("x-forwarded-for");
    if !dst.contains_key(&x_forwarded_for) {
        dst.insert(x_forwarded_for, HeaderValue::from_static("unknown"));
    }
}

pub fn forward_response_headers(src: &HeaderMap, dst: &mut HeaderMap) {
    for (name, value) in src {
        if !is_hop_by_hop_header(name.as_str()) {
            dst.append(name.clone(), value.clone());
        }
    }
}

pub fn build_upstream_url(base: &str, path_and_query: &str) -> Result<Uri, http::Error> {
    let base_uri: Uri = base.parse()?;
    let mut builder = Uri::builder();

    if let Some(scheme) = base_uri.scheme_str() {
        builder = builder.scheme(scheme);
    }

    if let Some(authority) = base_uri.authority() {
        builder = builder.authority(authority.as_str());
    }

    let base_path = base_uri.path();
    let (path_part, query_part) = split_path_and_query(path_and_query);

    let mut joined_path = join_paths(base_path, path_part);
    if let Some(query) = query_part {
        joined_path.push('?');
        joined_path.push_str(query);
    }

    builder.path_and_query(joined_path).build()
}

fn split_path_and_query(path_and_query: &str) -> (&str, Option<&str>) {
    if path_and_query.is_empty() {
        return ("", None);
    }

    if let Some(query) = path_and_query.strip_prefix('?') {
        return ("", Some(query));
    }

    match path_and_query.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (path_and_query, None),
    }
}

fn join_paths(base_path: &str, path_part: &str) -> String {
    let normalized_base = if base_path.is_empty() { "/" } else { base_path };

    if path_part.is_empty() {
        return normalized_base.to_string();
    }

    if normalized_base.ends_with('/') && path_part.starts_with('/') {
        format!("{}{}", normalized_base.trim_end_matches('/'), path_part)
    } else if !normalized_base.ends_with('/') && !path_part.starts_with('/') {
        if normalized_base == "/" {
            format!("/{}", path_part)
        } else {
            format!("{}/{}", normalized_base, path_part)
        }
    } else {
        format!("{}{}", normalized_base, path_part)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &'static str) -> HeaderName {
        HeaderName::from_static(name)
    }

    fn v(value: &'static str) -> HeaderValue {
        HeaderValue::from_static(value)
    }

    #[test]
    fn detects_hop_by_hop_headers_case_insensitively() {
        assert!(is_hop_by_hop_header("connection"));
        assert!(is_hop_by_hop_header("Connection"));
        assert!(is_hop_by_hop_header("TRANSFER-ENCODING"));
        assert!(!is_hop_by_hop_header("content-type"));
    }

    #[test]
    fn filters_hop_by_hop_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(h("connection"), v("keep-alive"));
        headers.insert(h("transfer-encoding"), v("chunked"));
        headers.insert(h("content-type"), v("application/json"));

        filter_hop_by_hop_headers(&mut headers);

        assert!(!headers.contains_key("connection"));
        assert!(!headers.contains_key("transfer-encoding"));
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
    }

    #[test]
    fn sets_and_updates_host_header() {
        let mut headers = HeaderMap::new();
        set_host_header(&mut headers, "example.com");
        assert_eq!(headers.get(HOST).unwrap(), "example.com");

        set_host_header(&mut headers, "upstream.internal:8443");
        assert_eq!(headers.get(HOST).unwrap(), "upstream.internal:8443");
    }

    #[test]
    fn forward_request_headers_copies_non_hop_by_hop_and_sets_host_and_xff() {
        let mut src = HeaderMap::new();
        src.insert(h("content-type"), v("application/json"));
        src.insert(h("connection"), v("keep-alive"));

        let mut dst = HeaderMap::new();
        forward_request_headers(&src, &mut dst, "api.example.com");

        assert_eq!(dst.get("content-type").unwrap(), "application/json");
        assert!(!dst.contains_key("connection"));
        assert_eq!(dst.get(HOST).unwrap(), "api.example.com");
        assert_eq!(dst.get("x-forwarded-for").unwrap(), "unknown");
    }

    #[test]
    fn forward_request_headers_preserves_existing_xff() {
        let mut src = HeaderMap::new();
        src.insert(h("x-forwarded-for"), v("203.0.113.10"));
        src.insert(h("accept"), v("*/*"));

        let mut dst = HeaderMap::new();
        forward_request_headers(&src, &mut dst, "api.example.com");

        assert_eq!(dst.get("x-forwarded-for").unwrap(), "203.0.113.10");
        assert_eq!(dst.get(HOST).unwrap(), "api.example.com");
        assert_eq!(dst.get("accept").unwrap(), "*/*");
    }

    #[test]
    fn forward_response_headers_copies_only_non_hop_by_hop() {
        let mut src = HeaderMap::new();
        src.insert(h("content-length"), v("42"));
        src.insert(h("connection"), v("close"));
        src.insert(h("upgrade"), v("h2c"));

        let mut dst = HeaderMap::new();
        forward_response_headers(&src, &mut dst);

        assert_eq!(dst.get("content-length").unwrap(), "42");
        assert!(!dst.contains_key("connection"));
        assert!(!dst.contains_key("upgrade"));
    }

    #[test]
    fn build_upstream_url_joins_with_single_slash() {
        let uri = build_upstream_url("https://example.com", "/v1/users?id=1").unwrap();
        assert_eq!(uri.to_string(), "https://example.com/v1/users?id=1");
    }

    #[test]
    fn build_upstream_url_joins_when_base_has_path() {
        let uri = build_upstream_url("https://example.com/api", "v1/users").unwrap();
        assert_eq!(uri.to_string(), "https://example.com/api/v1/users");
    }

    #[test]
    fn build_upstream_url_handles_double_slash_boundary() {
        let uri = build_upstream_url("https://example.com/api/", "/v1/users").unwrap();
        assert_eq!(uri.to_string(), "https://example.com/api/v1/users");
    }

    #[test]
    fn build_upstream_url_preserves_base_path_for_query_only_input() {
        let uri = build_upstream_url("https://example.com/api", "?active=true").unwrap();
        assert_eq!(uri.to_string(), "https://example.com/api?active=true");
    }

    #[test]
    fn build_upstream_url_returns_error_for_invalid_base() {
        assert!(build_upstream_url("not a uri", "/x").is_err());
    }
}
