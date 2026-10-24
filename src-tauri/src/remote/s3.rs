//! A read-only S3 client: a folder's objects and subfolders (ListObjectsV2, delimited by "/"), HEAD and
//! GET, signed with AWS Signature Version 4. Enough for Amazon S3 and the services that speak its API
//! (Cloudflare R2, Backblaze B2, Wasabi, MinIO…). With no access key, requests go unsigned: a public
//! bucket's.

use super::{http_failure, time, xml_items, Item};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use sha2::{Digest, Sha256};

/// What SigV4 leaves as it is: letters, digits and - _ . ~ (everything else is %XX, upper case).
const ENCODED: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// The SHA-256 of nothing: a GET's or a HEAD's payload.
const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// More than this in one folder isn't listed (a bucket can hold billions).
const MOST: usize = 200_000;

pub struct Bucket {
    pub client: reqwest::Client,
    /// Where requests go: "https://s3.us-east-1.amazonaws.com", "https://<account>.r2.cloudflarestorage.com", "http://127.0.0.1:9000"…
    pub endpoint: url::Url,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret: String,
    /// What the folder it starts at starts its keys with, "" for the bucket's top; no "/" at either end.
    pub root: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// HMAC-SHA256 (RFC 2104), on the sha2 already built.
pub(crate) fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(data);
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn encode(text: &str) -> String {
    utf8_percent_encode(text, ENCODED).to_string()
}

/// A key's path, each part encoded and the slashes kept: how S3 wants it signed (only once).
fn encode_key(key: &str) -> String {
    key.split('/').map(encode).collect::<Vec<_>>().join("/")
}

/// A key as a `encoding-type=url` listing gives it.
fn decode_key(key: &str) -> String {
    percent_decode_str(&key.replace('+', " ")).decode_utf8_lossy().into_owned()
}

/// Who signs, and for which region.
struct Signer<'a> {
    access_key: &'a str,
    secret: &'a str,
    region: &'a str,
}

impl Signer<'_> {
    /// The Authorization header SigV4 asks for, for a request with the payload hash `payload`;
    /// `headers` are every header signed (lower-case names, host and x-amz-* among them), sorted.
    fn authorization(&self, method: &str, path: &str, query: &str, headers: &[(&str, &str)], payload: &str, amz_date: &str) -> String {
        let canonical: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
        let signed = headers.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(";");
        let request = format!("{method}\n{path}\n{query}\n{canonical}\n{signed}\n{payload}");
        let date = &amz_date[..8];
        let scope = format!("{date}/{}/s3/aws4_request", self.region);
        let to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", hex(&Sha256::digest(request.as_bytes())));
        let key = [date, self.region, "s3", "aws4_request"].iter().fold(format!("AWS4{}", self.secret).into_bytes(), |key, part| hmac(&key, part.as_bytes()).to_vec());
        format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={}", self.access_key, hex(&hmac(&key, to_sign.as_bytes())))
    }
}

impl Bucket {
    /// Amazon's own endpoints take the bucket in the host name, as Amazon prefers; the rest take it in
    /// the path, which all of them do.
    fn virtual_host(&self) -> bool {
        self.endpoint.host_str().is_some_and(|h| h.ends_with(".amazonaws.com")) && !self.bucket.contains('.')
    }

    /// The whole key for `path` (below the folder it starts at).
    fn key(&self, path: &str) -> String {
        [self.root.as_str(), path.trim_matches('/')].iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join("/")
    }

    /// Sends a GET or HEAD for `key` ("" for the bucket itself) with `query` (unencoded pairs).
    async fn send(&self, method: reqwest::Method, key: &str, query: &[(&str, &str)], range: Option<&str>) -> Result<reqwest::Response, String> {
        let host = match (self.endpoint.host_str(), self.endpoint.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            (None, _) => return Err("The S3 address has no host".into()),
        };
        let (host, path) = if self.virtual_host() {
            (format!("{}.{host}", self.bucket), format!("/{}", encode_key(key)))
        } else {
            let base = self.endpoint.path().trim_end_matches('/');
            (host, format!("{base}/{}{}", encode(&self.bucket), if key.is_empty() { String::new() } else { format!("/{}", encode_key(key)) }))
        };
        let mut pairs: Vec<(String, String)> = query.iter().map(|(k, v)| (encode(k), encode(v))).collect();
        pairs.sort();
        let query = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
        let url = format!("{}://{host}{path}{}{query}", self.endpoint.scheme(), if query.is_empty() { "" } else { "?" });
        let mut request = self.client.request(method.clone(), &url);
        if let Some(range) = range {
            request = request.header("range", range);
        }
        if !self.access_key.is_empty() {
            let amz_date = time::amz_date(std::time::SystemTime::now());
            let mut headers = vec![("host", host.as_str()), ("x-amz-content-sha256", EMPTY), ("x-amz-date", amz_date.as_str())];
            if let Some(range) = range {
                headers.push(("range", range));
            }
            headers.sort();
            let signer = Signer { access_key: &self.access_key, secret: &self.secret, region: &self.region };
            let auth = signer.authorization(method.as_str(), &path, &query, &headers, EMPTY, &amz_date);
            request = request.header("x-amz-content-sha256", EMPTY).header("x-amz-date", &amz_date).header("authorization", auth);
        }
        let response = request.send().await.map_err(|e| super::reach_failure(&e))?;
        if response.status().is_success() || response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            return Ok(response);
        }
        let status = response.status();
        let region = response.headers().get("x-amz-bucket-region").and_then(|v| v.to_str().ok()).map(str::to_string);
        let body = if method == reqwest::Method::HEAD { String::new() } else { response.text().await.unwrap_or_default() };
        Err(s3_failure(status, &body, region.as_deref(), &self.bucket))
    }

    /// The objects and subfolders in the folder `path` ("" for the folder it starts at).
    pub async fn list(&self, path: &str) -> Result<Vec<Item>, String> {
        let mut prefix = self.key(path);
        if !prefix.is_empty() {
            prefix.push('/');
        }
        let mut items = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type", "2"), ("delimiter", "/"), ("encoding-type", "url"), ("max-keys", "1000"), ("prefix", prefix.as_str())];
            if let Some(token) = &token {
                query.push(("continuation-token", token));
            }
            let body = self.send(reqwest::Method::GET, "", &query, None).await?.text().await.map_err(|e| e.to_string())?;
            let page = parse_list(&body, &prefix);
            items.extend(page.items);
            match page.next {
                Some(next) if items.len() < MOST => token = Some(next),
                _ => break,
            }
        }
        Ok(items)
    }

    /// The object at `path`, or the folder (a prefix with something under it); None when neither.
    pub async fn stat(&self, path: &str) -> Result<Option<Item>, String> {
        let key = self.key(path);
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        if key.is_empty() {
            return Ok(Some(Item { name, dir: true, size: 0, modified: 0 }));
        }
        match self.send(reqwest::Method::HEAD, &key, &[], None).await {
            Ok(response) => {
                let size = response.headers().get("content-length").and_then(|v| v.to_str().ok()?.parse().ok()).unwrap_or(0);
                let modified = response.headers().get("last-modified").and_then(|v| time::http_date(v.to_str().ok()?)).unwrap_or(0);
                return Ok(Some(Item { name, dir: false, size, modified }));
            }
            Err(e) if e != NOT_FOUND => return Err(e),
            Err(_) => {}
        }
        let prefix = format!("{key}/");
        let body = self
            .send(reqwest::Method::GET, "", &[("list-type", "2"), ("delimiter", "/"), ("max-keys", "1"), ("prefix", prefix.as_str())], None)
            .await?
            .text()
            .await
            .map_err(|e| e.to_string())?;
        let page = parse_list(&body, &prefix);
        Ok((page.any || !page.items.is_empty()).then_some(Item { name, dir: true, size: 0, modified: 0 }))
    }

    /// The object at `path` (its first `first` bytes, when that's given), to read as it comes.
    pub async fn get(&self, path: &str, first: Option<u64>) -> Result<reqwest::Response, String> {
        let range = first.map(|n| format!("bytes=0-{}", n.saturating_sub(1)));
        self.send(reqwest::Method::GET, &self.key(path), &[], range.as_deref()).await
    }
}

pub(super) const NOT_FOUND: &str = "It isn't there (any more)";

/// What an S3 error says, in words: its XML's code and message.
fn s3_failure(status: reqwest::StatusCode, body: &str, region: Option<&str>, bucket: &str) -> String {
    let fields = xml_items(body, "Error", &["Code", "Message"]);
    let (code, message) = fields.first().map_or((String::new(), String::new()), |f| (f.text(0).to_string(), f.text(1).to_string()));
    match (status.as_u16(), code.as_str()) {
        (404, "NoSuchBucket") => format!("There's no bucket called “{bucket}” there"),
        (404, _) => NOT_FOUND.into(),
        (416, _) => super::UNSATISFIABLE.into(),
        (301, _) | (_, "PermanentRedirect" | "AuthorizationHeaderMalformed") if region.is_some() => format!("The bucket is in the region {}: set that in the connection", region.unwrap_or_default()),
        (_, "InvalidAccessKeyId" | "SignatureDoesNotMatch") => "The access key or its secret isn't right".into(),
        (403, _) => format!("Access denied{}", if message.is_empty() { String::new() } else { format!(": {message}") }),
        _ => http_failure(status, if message.is_empty() { &code } else { &message }),
    }
}

struct Page {
    items: Vec<Item>,
    next: Option<String>,
    /// Anything at all was under the prefix (the folder's own marker object included).
    any: bool,
}

/// A ListObjectsV2 page (keys URL-encoded): what's directly in the folder `prefix`, and where the next
/// page starts.
fn parse_list(body: &str, prefix: &str) -> Page {
    let mut items = Vec::new();
    let mut any = false;
    for object in xml_items(body, "Contents", &["Key", "Size", "LastModified"]) {
        any = true;
        let key = decode_key(&object[0]);
        let Some(name) = key.strip_prefix(prefix) else { continue };
        // The folder's own "prefix/" marker, and anything a delimiter should have folded away.
        if name.is_empty() || name.contains('/') {
            continue;
        }
        items.push(Item { name: name.to_string(), dir: false, size: object[1].parse().unwrap_or(0), modified: time::iso_date(&object[2]).unwrap_or(0) });
    }
    for folder in xml_items(body, "CommonPrefixes", &["Prefix"]) {
        any = true;
        let key = decode_key(&folder[0]);
        let Some(name) = key.strip_prefix(prefix).map(|n| n.trim_end_matches('/')) else { continue };
        if !name.is_empty() && !name.contains('/') {
            items.push(Item { name: name.to_string(), dir: true, size: 0, modified: 0 });
        }
    }
    let truncated = xml_items(body, "ListBucketResult", &["IsTruncated", "NextContinuationToken"]);
    let next = truncated.first().filter(|f| f.text(0) == "true" && !f.text(1).is_empty()).map(|f| f.text(1).to_string());
    Page { items, next, any }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_as_aws_says() {
        // RFC 4231, test case 2.
        assert_eq!(hex(&hmac(b"Jefe", b"what do ya want for nothing?")), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
        let signer = Signer { access_key: "AKIAIOSFODNN7EXAMPLE", secret: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", region: "us-east-1" };
        // Amazon's "GET Object" example (with a Range header) and its "GET Bucket (List Objects)" one.
        let headers = [("host", "examplebucket.s3.amazonaws.com"), ("range", "bytes=0-9"), ("x-amz-content-sha256", EMPTY), ("x-amz-date", "20130524T000000Z")];
        let auth = signer.authorization("GET", "/test.txt", "", &headers, EMPTY, "20130524T000000Z");
        assert!(auth.ends_with("SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"), "{auth}");
        let headers = [("host", "examplebucket.s3.amazonaws.com"), ("x-amz-content-sha256", EMPTY), ("x-amz-date", "20130524T000000Z")];
        let auth = signer.authorization("GET", "/", "max-keys=2&prefix=J", &headers, EMPTY, "20130524T000000Z");
        assert!(auth.ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"), "{auth}");
        assert_eq!(encode_key("photos/2024 trip/a+b (1).jpg"), "photos/2024%20trip/a%2Bb%20%281%29.jpg");
    }

    #[test]
    fn reads_a_listing() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>b</Name><Prefix>photos%2F</Prefix><KeyCount>4</KeyCount><IsTruncated>true</IsTruncated><EncodingType>url</EncodingType>
<NextContinuationToken>1ueGcxLPRx1Tr/XYExHnhbYLgveDs2J/wm36Hy4vbOwM=</NextContinuationToken>
<Contents><Key>photos%2F</Key><Size>0</Size><LastModified>2024-01-01T00:00:00.000Z</LastModified></Contents>
<Contents><Key>photos%2Fbeach+day.jpg</Key><LastModified>2024-07-14T10:30:05.250Z</LastModified><Size>204800</Size><StorageClass>STANDARD</StorageClass></Contents>
<Contents><Key>photos%2Fa%3Ab%26c.txt</Key><LastModified>2024-07-14T10:30:05Z</LastModified><Size>3</Size></Contents>
<CommonPrefixes><Prefix>photos%2F2023%2F</Prefix></CommonPrefixes></ListBucketResult>"#;
        let page = parse_list(body, "photos/");
        let names: Vec<(&str, bool, u64)> = page.items.iter().map(|i| (i.name.as_str(), i.dir, i.size)).collect();
        assert_eq!(names, [("beach day.jpg", false, 204800), ("a:b&c.txt", false, 3), ("2023", true, 0)]);
        assert_eq!(page.items[0].modified, 1_720_953_005_250);
        assert_eq!(page.next.as_deref(), Some("1ueGcxLPRx1Tr/XYExHnhbYLgveDs2J/wm36Hy4vbOwM="));
        assert!(page.any);
        let error = "<?xml version=\"1.0\"?><Error><Code>NoSuchBucket</Code><Message>The specified bucket does not exist</Message></Error>";
        assert_eq!(s3_failure(reqwest::StatusCode::NOT_FOUND, error, None, "photos"), "There's no bucket called “photos” there");
    }
}
