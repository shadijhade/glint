//! A read-only WebDAV client: a folder's members (PROPFIND, depth 1), one item's (depth 0), and GET,
//! with basic authentication when there's a user name. Nextcloud, ownCloud, a NAS's WebDAV, dufs…

use super::{http_failure, time, xml_items, Item};
use base64::Engine;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};

/// What's encoded in a path's parts: what a URL's path can't hold as it is.
const PART: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'#').add(b'%').add(b'<').add(b'>').add(b'?').add(b'`').add(b'{').add(b'}').add(b'/').add(b'\\').add(b'[').add(b']').add(b'^').add(b'|');

const PROPS: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/><d:getlastmodified/></d:prop></d:propfind>"#;

/// More than this in one folder isn't listed.
const MOST: usize = 200_000;

pub struct Server {
    pub client: reqwest::Client,
    /// The server's address, its path ending with "/".
    pub base: url::Url,
    pub user: String,
    pub password: String,
    /// The folder it starts at, below `base` ("" for `base` itself); no "/" at either end.
    pub root: String,
}

impl Server {
    /// The URL of `path` (below the folder it starts at), a folder's with a trailing "/".
    fn url(&self, path: &str, folder: bool) -> String {
        let parts: Vec<String> = [self.root.as_str(), path].iter().flat_map(|p| p.split('/')).filter(|p| !p.is_empty()).map(|p| utf8_percent_encode(p, PART).to_string()).collect();
        let mut url = format!("{}{}", self.base, parts.join("/"));
        if folder && !url.ends_with('/') {
            url.push('/');
        }
        url
    }

    async fn send(&self, method: reqwest::Method, url: &str, depth: Option<&str>, first: Option<u64>) -> Result<reqwest::Response, String> {
        let mut request = self.client.request(method.clone(), url);
        if let Some(n) = first {
            request = request.header("range", format!("bytes=0-{}", n.saturating_sub(1)));
        }
        if !self.user.is_empty() {
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", self.user, self.password));
            request = request.header("authorization", format!("Basic {token}"));
        }
        if let Some(depth) = depth {
            request = request.header("depth", depth).header("content-type", "application/xml; charset=utf-8").body(PROPS);
        }
        let response = request.send().await.map_err(|e| super::reach_failure(&e))?;
        let status = response.status();
        if status.is_success() || status.as_u16() == 207 {
            return Ok(response);
        }
        Err(match status.as_u16() {
            401 => "The server didn't take the user name and password".into(),
            403 => "The server won't let this user in there".into(),
            404 => super::s3::NOT_FOUND.into(),
            416 => super::UNSATISFIABLE.into(),
            405 if depth.is_some() => "That address doesn't speak WebDAV (it refused PROPFIND)".into(),
            _ => http_failure(status, ""),
        })
    }

    async fn propfind(&self, path: &str, depth: &str) -> Result<Vec<(String, Item)>, String> {
        let url = self.url(path, depth == "1");
        let method = reqwest::Method::from_bytes(b"PROPFIND").map_err(|e| e.to_string())?;
        let body = self.send(method, &url, Some(depth), None).await?.text().await.map_err(|e| e.to_string())?;
        Ok(members(&body))
    }

    /// What's in the folder `path` ("" for the folder it starts at).
    pub async fn list(&self, path: &str) -> Result<Vec<Item>, String> {
        let here = url::Url::parse(&self.url(path, true)).map_err(|e| e.to_string())?;
        let here = decoded(here.path());
        let mut items: Vec<Item> = self.propfind(path, "1").await?.into_iter().filter(|(href, _)| decoded(&path_of(href)) != here).map(|(_, item)| item).collect();
        items.retain(|i| !i.name.is_empty());
        items.truncate(MOST);
        Ok(items)
    }

    pub async fn stat(&self, path: &str) -> Result<Option<Item>, String> {
        match self.propfind(path, "0").await {
            Ok(found) => Ok(found.into_iter().next().map(|(_, mut item)| {
                item.name = path.rsplit('/').next().unwrap_or(path).to_string();
                item
            })),
            Err(e) if e == super::s3::NOT_FOUND => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The file at `path` (its first `first` bytes, when that's given and the server takes ranges).
    pub async fn get(&self, path: &str, first: Option<u64>) -> Result<reqwest::Response, String> {
        self.send(reqwest::Method::GET, &self.url(path, false), None, first).await
    }
}

/// An href's path (an href may be a whole URL), without a trailing "/".
fn path_of(href: &str) -> String {
    let path = url::Url::parse(href).map(|u| u.path().to_string()).unwrap_or_else(|_| href.to_string());
    path.trim_end_matches('/').to_string()
}

fn decoded(path: &str) -> String {
    percent_decode_str(path.trim_end_matches('/')).decode_utf8_lossy().into_owned()
}

/// A multistatus answer's members: each one's href, and what it is.
fn members(body: &str) -> Vec<(String, Item)> {
    xml_items(body, "response", &["href", "collection", "getcontentlength", "getlastmodified"])
        .into_iter()
        .filter(|f| !f.text(0).is_empty())
        .map(|f| {
            let href = f.text(0).to_string();
            let path = decoded(&path_of(&href));
            let name = path.rsplit('/').next().unwrap_or("").to_string();
            let dir = f.has(1);
            let item = Item { name, dir, size: if dir { 0 } else { f.text(2).trim().parse().unwrap_or(0) }, modified: time::http_date(f.text(3)).unwrap_or(0) };
            (href, item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_multistatus() {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
<D:response><D:href>/dav/photos/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype><D:getlastmodified>Sun, 14 Jul 2024 10:30:05 GMT</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
<D:response><D:href>/dav/photos/beach%20day.jpg</D:href><D:propstat><D:prop><D:resourcetype/><D:getcontentlength>204800</D:getcontentlength><D:getlastmodified>Sun, 14 Jul 2024 10:30:05 GMT</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
<D:response><D:href>http://nas.local/dav/photos/2023/</D:href><D:propstat><D:prop><D:resourcetype><D:collection></D:collection></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
<d:response xmlns:d="DAV:"><d:href>/dav/photos/r%C3%A9sum%C3%A9%20%26%20notes.txt</d:href><d:propstat><d:prop><d:getcontentlength>12</d:getcontentlength></d:prop></d:propstat></d:response>
</D:multistatus>"#;
        let found = members(body);
        let names: Vec<(&str, bool, u64)> = found.iter().map(|(_, i)| (i.name.as_str(), i.dir, i.size)).collect();
        assert_eq!(names, [("photos", true, 0), ("beach day.jpg", false, 204800), ("2023", true, 0), ("résumé & notes.txt", false, 12)]);
        assert_eq!(found[1].1.modified, 1_720_953_005_000);
        let server = Server { client: super::super::client().unwrap(), base: url::Url::parse("https://nas.local/dav/").unwrap(), user: String::new(), password: String::new(), root: "photos".into() };
        assert_eq!(server.url("2024 trip/a#1.jpg", false), "https://nas.local/dav/photos/2024%20trip/a%231.jpg");
        assert_eq!(server.url("", true), "https://nas.local/dav/photos/");
    }
}
