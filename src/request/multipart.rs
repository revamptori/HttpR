//! Multipart/form-data request body serialization (RFC 7578).

use bytes::{BufMut, Bytes, BytesMut};
use rand::Rng;

/// Represents an uploaded file part in a multipart/form-data request.
#[derive(Debug, Clone)]
pub struct MultipartFile {
    pub field_name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub content: Bytes,
    pub headers: Vec<(String, String)>,
}

/// Represents a text form field part in a multipart/form-data request.
#[derive(Debug, Clone)]
pub struct MultipartField {
    pub name: String,
    pub value: String,
}

/// Encodes form fields and file parts into a `multipart/form-data` payload.
#[derive(Debug, Clone)]
pub struct MultipartEncoder {
    boundary: String,
    fields: Vec<MultipartField>,
    files: Vec<MultipartFile>,
}

impl MultipartEncoder {
    /// Creates a new [`MultipartEncoder`] with a cryptographically random boundary.
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let rand_bytes: [u8; 16] = rng.gen();
        let boundary = format!("httpr-{}", hex::encode(rand_bytes));
        Self {
            boundary,
            fields: Vec::new(),
            files: Vec::new(),
        }
    }

    /// Returns the generated multipart boundary string.
    pub fn boundary(&self) -> &str {
        &self.boundary
    }

    /// Returns the full `Content-Type` header value including boundary.
    pub fn content_type(&self) -> String {
        format!("multipart/form-data; boundary={}", self.boundary)
    }

    /// Adds a plain form field.
    pub fn add_field(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.fields.push(MultipartField {
            name: name.into(),
            value: value.into(),
        });
    }

    /// Adds a file part.
    pub fn add_file(&mut self, file: MultipartFile) {
        self.files.push(file);
    }

    /// Encodes all parts into a contiguous zero-copy-ready [`Bytes`] buffer.
    pub fn encode(&self) -> Bytes {
        let estimated_size: usize = self
            .fields
            .iter()
            .map(|f| f.name.len() + f.value.len() + 128)
            .sum::<usize>()
            + self
                .files
                .iter()
                .map(|f| f.content.len() + 256)
                .sum::<usize>()
            + 128;

        let mut buf = BytesMut::with_capacity(estimated_size);

        for field in &self.fields {
            buf.put_slice(b"--");
            buf.put_slice(self.boundary.as_bytes());
            buf.put_slice(b"\r\nContent-Disposition: form-data; name=\"");
            write_escaped_quoted(&mut buf, &field.name);
            buf.put_slice(b"\"\r\n\r\n");
            buf.put_slice(field.value.as_bytes());
            buf.put_slice(b"\r\n");
        }

        for file in &self.files {
            buf.put_slice(b"--");
            buf.put_slice(self.boundary.as_bytes());
            buf.put_slice(b"\r\nContent-Disposition: form-data; name=\"");
            write_escaped_quoted(&mut buf, &file.field_name);
            buf.put_slice(b"\"");
            if let Some(filename) = &file.filename {
                buf.put_slice(b"; filename=\"");
                write_escaped_quoted(&mut buf, filename);
                buf.put_slice(b"\"");
            }
            buf.put_slice(b"\r\n");

            if let Some(ct) = &file.content_type {
                buf.put_slice(b"Content-Type: ");
                buf.put_slice(ct.as_bytes());
                buf.put_slice(b"\r\n");
            } else if file.filename.is_some() {
                buf.put_slice(b"Content-Type: application/octet-stream\r\n");
            }

            for (hk, hv) in &file.headers {
                buf.put_slice(hk.as_bytes());
                buf.put_slice(b": ");
                buf.put_slice(hv.as_bytes());
                buf.put_slice(b"\r\n");
            }

            buf.put_slice(b"\r\n");
            buf.put_slice(&file.content);
            buf.put_slice(b"\r\n");
        }

        buf.put_slice(b"--");
        buf.put_slice(self.boundary.as_bytes());
        buf.put_slice(b"--\r\n");

        buf.freeze()
    }
}

impl Default for MultipartEncoder {
    fn default() -> Self {
        Self::new()
    }
}

fn write_escaped_quoted(buf: &mut BytesMut, value: &str) {
    for ch in value.bytes() {
        match ch {
            b'"' => buf.put_slice(b"%22"),
            b'\r' => buf.put_slice(b"%0D"),
            b'\n' => buf.put_slice(b"%0A"),
            _ => buf.put_u8(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multipart_encode() {
        let mut enc = MultipartEncoder::new();
        enc.add_field("field1", "hello");
        enc.add_file(MultipartFile {
            field_name: "upload".to_string(),
            filename: Some("test.txt".to_string()),
            content_type: Some("text/plain".to_string()),
            content: Bytes::from_static(b"file-body"),
            headers: vec![],
        });
        let encoded = enc.encode();
        let text = String::from_utf8_lossy(&encoded);
        assert!(text.contains("name=\"field1\""));
        assert!(text.contains("filename=\"test.txt\""));
        assert!(text.contains("file-body"));
    }
}
