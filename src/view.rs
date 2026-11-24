//! Static browser assets embedded in the standalone binary.
pub fn index_html() -> &'static str {
    include_str!("web/dashboard.html")
}

pub fn asset(path: &str) -> Option<(&'static str, &'static str)> {
    match path {
        "/assets/dashboard.css" => {
            Some(("text/css; charset=utf-8", include_str!("web/dashboard.css")))
        }
        "/assets/activity.js" => Some((
            "text/javascript; charset=utf-8",
            include_str!("web/activity.js"),
        )),
        "/assets/dashboard.js" => Some((
            "text/javascript; charset=utf-8",
            include_str!("web/dashboard.js"),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_loads_its_embedded_assets() {
        for path in [
            "/assets/dashboard.css",
            "/assets/activity.js",
            "/assets/dashboard.js",
        ] {
            assert!(index_html().contains(path));
            assert!(asset(path).is_some_and(|(_, body)| !body.is_empty()));
        }
        assert!(asset("/assets/missing.js").is_none());
    }
}
