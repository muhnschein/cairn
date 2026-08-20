//! Builds small ZIM archives for tests.
//!
//! Test-only: nothing in cairn writes ZIM files.

#![forbid(unsafe_code)]

/// ZIM magic number.
pub const MAGIC: u32 = 72_173_914;

/// Cluster compression to use for content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Store cluster bodies as-is.
    None,
    /// LZMA2 in an xz container.
    Xz,
    /// Zstandard.
    Zstd,
}

impl Compression {
    fn tag(self) -> u8 {
        match self {
            Compression::None => 1,
            Compression::Xz => 4,
            Compression::Zstd => 5,
        }
    }
}

#[derive(Debug, Clone)]
enum Kind {
    Content {
        mime: u16,
        data: Vec<u8>,
    },
    Redirect {
        to_ns: u8,
        to_url: String,
    },
    /// `X/listing/titleOrdered/v1`: filled in once entry indices are fixed.
    TitleListing,
}

#[derive(Debug, Clone)]
struct Entry {
    ns: u8,
    url: String,
    title: String,
    kind: Kind,
}

/// Assembles an archive from entries added in any order.
#[derive(Debug, Clone)]
pub struct Builder {
    uuid: [u8; 16],
    major: u16,
    minor: u16,
    mimes: Vec<String>,
    entries: Vec<Entry>,
    compression: Compression,
    extended: bool,
    blobs_per_cluster: usize,
    checksum: bool,
    main_page: Option<String>,
    title_listing: bool,
}

impl Default for Builder {
    fn default() -> Self {
        Builder::new()
    }
}

impl Builder {
    /// A version 6.1 archive with one MIME type and no entries.
    pub fn new() -> Builder {
        Builder {
            uuid: *b"cairn-test-uuid1",
            major: 6,
            minor: 1,
            mimes: vec!["text/html".into()],
            entries: Vec::new(),
            compression: Compression::None,
            extended: false,
            blobs_per_cluster: 4,
            checksum: true,
            main_page: None,
            title_listing: true,
        }
    }

    /// Set the archive UUID.
    pub fn uuid(mut self, uuid: [u8; 16]) -> Builder {
        self.uuid = uuid;
        self
    }

    /// Set the format version. `(6, 1)` selects the new namespace scheme.
    pub fn version(mut self, major: u16, minor: u16) -> Builder {
        self.major = major;
        self.minor = minor;
        self
    }

    /// Set cluster compression.
    pub fn compression(mut self, c: Compression) -> Builder {
        self.compression = c;
        self
    }

    /// Use 64-bit blob offsets.
    pub fn extended(mut self, yes: bool) -> Builder {
        self.extended = yes;
        self
    }

    /// Blobs packed into one cluster.
    pub fn blobs_per_cluster(mut self, n: usize) -> Builder {
        self.blobs_per_cluster = n.max(1);
        self
    }

    /// Keep the title ordering in the header, as archives written before
    /// libzim moved it into an entry did. Current libzim writes the sentinel
    /// and a listing entry, which is what this builder does by default.
    pub fn legacy_title_index(mut self) -> Builder {
        self.title_listing = false;
        self
    }

    /// Append a 16-byte checksum block.
    pub fn checksum(mut self, yes: bool) -> Builder {
        self.checksum = yes;
        self
    }

    /// Declare the MIME table.
    pub fn mimes<I: IntoIterator<Item = S>, S: Into<String>>(mut self, m: I) -> Builder {
        self.mimes = m.into_iter().map(Into::into).collect();
        self
    }

    /// Mark `url` in the content namespace as the main page.
    pub fn main_page(mut self, url: &str) -> Builder {
        self.main_page = Some(url.into());
        self
    }

    /// Namespace this archive stores content in.
    pub fn content_ns(&self) -> u8 {
        if self.major >= 6 && self.minor >= 1 {
            b'C'
        } else {
            b'A'
        }
    }

    /// Add a content entry in the content namespace.
    pub fn content(self, url: &str, title: &str, mime: u16, data: &[u8]) -> Builder {
        let ns = self.content_ns();
        self.entry_in(
            ns,
            url,
            title,
            Kind::Content {
                mime,
                data: data.to_vec(),
            },
        )
    }

    /// Add a content entry in an explicit namespace.
    pub fn content_in(self, ns: u8, url: &str, title: &str, mime: u16, data: &[u8]) -> Builder {
        self.entry_in(
            ns,
            url,
            title,
            Kind::Content {
                mime,
                data: data.to_vec(),
            },
        )
    }

    /// Add a redirect in the content namespace.
    pub fn redirect(self, url: &str, title: &str, to_url: &str) -> Builder {
        let ns = self.content_ns();
        self.entry_in(
            ns,
            url,
            title,
            Kind::Redirect {
                to_ns: ns,
                to_url: to_url.into(),
            },
        )
    }

    fn entry_in(mut self, ns: u8, url: &str, title: &str, kind: Kind) -> Builder {
        self.entries.push(Entry {
            ns,
            url: url.into(),
            title: title.into(),
            kind,
        });
        self
    }

    /// Serialize the archive.
    pub fn build(&self) -> Vec<u8> {
        let content_ns = self.content_ns();
        let mut mimes = self.mimes.clone();

        let mut entries = self.entries.clone();
        if self.title_listing {
            // libzim adds this entry before assigning indices, and so must we:
            // the payload it carries is a list of those indices.
            mimes.push("application/octet-stream+zimlisting".into());
            entries.push(Entry {
                ns: b'X',
                url: "listing/titleOrdered/v1".into(),
                title: String::new(),
                kind: Kind::TitleListing,
            });
        }
        entries.sort_by(|a, b| (a.ns, &a.url).cmp(&(b.ns, &b.url)));

        // Front articles, title-ordered, as little-endian entry indices.
        let listing_mime = mimes.len().saturating_sub(1) as u16;
        if self.title_listing {
            let mut front: Vec<u32> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.ns == content_ns)
                .map(|(i, _)| i as u32)
                .collect();
            front.sort_by_key(|&i| {
                let e = &entries[i as usize];
                if e.title.is_empty() {
                    e.url.clone()
                } else {
                    e.title.clone()
                }
            });
            let payload: Vec<u8> = front.iter().flat_map(|i| i.to_le_bytes()).collect();
            for e in entries.iter_mut() {
                if matches!(e.kind, Kind::TitleListing) {
                    e.kind = Kind::Content {
                        mime: listing_mime,
                        data: payload.clone(),
                    };
                    break;
                }
            }
        }
        let entries = entries;

        let index_of = |ns: u8, url: &str| -> u32 {
            entries
                .iter()
                .position(|e| e.ns == ns && e.url == url)
                .map(|i| i as u32)
                .unwrap_or(u32::MAX)
        };

        // Pack blobs into clusters. The listing goes into one of its own,
        // uncompressed: libzim requires that, and a reader must be able to
        // borrow the indices straight from the mapping.
        let mut clusters: Vec<(bool, Vec<Vec<u8>>)> = Vec::new();
        let mut placement: Vec<Option<(u32, u32)>> = Vec::new();
        let compressed = self.compression != Compression::None;
        for (i, e) in entries.iter().enumerate() {
            match &e.kind {
                Kind::Content { data, .. } => {
                    let listing =
                        self.title_listing && e.ns == b'X' && e.url.starts_with("listing/");
                    let fits = |c: &(bool, Vec<Vec<u8>>)| {
                        c.0 == (compressed && !listing) && c.1.len() < self.blobs_per_cluster
                    };
                    if listing || clusters.last().is_none_or(|c| !fits(c)) {
                        clusters.push((compressed && !listing, Vec::new()));
                    }
                    let ci = clusters.len() as u32 - 1;
                    let bi = clusters[ci as usize].1.len() as u32;
                    clusters[ci as usize].1.push(data.clone());
                    placement.push(Some((ci, bi)));
                    let _ = i;
                }
                Kind::Redirect { .. } | Kind::TitleListing => placement.push(None),
            }
        }

        let mut out = vec![0u8; 80];

        let mime_list_pos = out.len() as u64;
        for m in &mimes {
            out.extend_from_slice(m.as_bytes());
            out.push(0);
        }
        out.push(0);

        // Canonical layout: pointer lists first, dirents next, clusters last.
        let url_ptr_pos = out.len() as u64;
        out.extend(std::iter::repeat_n(0u8, entries.len() * 8));
        let title_ptr_pos = out.len() as u64;
        out.extend(std::iter::repeat_n(0u8, entries.len() * 4));
        let cluster_ptr_pos = out.len() as u64;
        out.extend(std::iter::repeat_n(0u8, clusters.len() * 8));

        let mut dirent_pos = Vec::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            dirent_pos.push(out.len() as u64);
            match &e.kind {
                Kind::Content { mime, .. } => {
                    let (ci, bi) = placement[i].unwrap_or((0, 0));
                    out.extend_from_slice(&mime.to_le_bytes());
                    out.push(0);
                    out.push(e.ns);
                    out.extend_from_slice(&0u32.to_le_bytes());
                    out.extend_from_slice(&ci.to_le_bytes());
                    out.extend_from_slice(&bi.to_le_bytes());
                }
                Kind::Redirect { to_ns, to_url } => {
                    out.extend_from_slice(&0xffffu16.to_le_bytes());
                    out.push(0);
                    out.push(e.ns);
                    out.extend_from_slice(&0u32.to_le_bytes());
                    out.extend_from_slice(&index_of(*to_ns, to_url).to_le_bytes());
                }
                // Resolved to content above, before indices were assigned.
                Kind::TitleListing => unreachable!("title listing was not filled in"),
            }
            out.extend_from_slice(e.url.as_bytes());
            out.push(0);
            out.extend_from_slice(e.title.as_bytes());
            out.push(0);
        }

        let mut cluster_pos = Vec::with_capacity(clusters.len());
        for (compressed, blobs) in &clusters {
            cluster_pos.push(out.len() as u64);
            let tag = if *compressed {
                self.compression.tag()
            } else {
                Compression::None.tag()
            };
            out.push(tag | if self.extended { 0x10 } else { 0 });
            let body = cluster_body(blobs, self.extended);
            match if *compressed {
                self.compression
            } else {
                Compression::None
            } {
                Compression::None => out.extend_from_slice(&body),
                Compression::Xz => {
                    let mut packed = Vec::new();
                    lzma_rs::xz_compress(&mut &body[..], &mut packed).expect("xz compress");
                    out.extend_from_slice(&packed);
                }
                Compression::Zstd => {
                    let packed = ruzstd::encoding::compress_to_vec(
                        &body[..],
                        ruzstd::encoding::CompressionLevel::Fastest,
                    );
                    out.extend_from_slice(&packed);
                }
            }
        }

        let mut by_title: Vec<u32> = (0..entries.len() as u32).collect();
        by_title.sort_by_key(|&i| {
            let e = &entries[i as usize];
            let t = if e.title.is_empty() {
                e.url.clone()
            } else {
                e.title.clone()
            };
            (e.ns, t)
        });

        for (i, p) in dirent_pos.iter().enumerate() {
            let at = url_ptr_pos as usize + i * 8;
            out[at..at + 8].copy_from_slice(&p.to_le_bytes());
        }
        for (i, e) in by_title.iter().enumerate() {
            let at = title_ptr_pos as usize + i * 4;
            out[at..at + 4].copy_from_slice(&e.to_le_bytes());
        }
        for (i, p) in cluster_pos.iter().enumerate() {
            let at = cluster_ptr_pos as usize + i * 8;
            out[at..at + 8].copy_from_slice(&p.to_le_bytes());
        }

        let checksum_pos = if self.checksum {
            let at = out.len() as u64;
            out.extend_from_slice(&[0u8; 16]);
            at
        } else {
            0
        };

        let main_page = self
            .main_page
            .as_deref()
            .map(|u| index_of(self.content_ns(), u))
            .unwrap_or(u32::MAX);

        let mut h = Vec::with_capacity(80);
        h.extend_from_slice(&MAGIC.to_le_bytes());
        h.extend_from_slice(&self.major.to_le_bytes());
        h.extend_from_slice(&self.minor.to_le_bytes());
        h.extend_from_slice(&self.uuid);
        h.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        h.extend_from_slice(&(clusters.len() as u32).to_le_bytes());
        h.extend_from_slice(&url_ptr_pos.to_le_bytes());
        // Current libzim writes -1 here and stores the ordering as an entry.
        h.extend_from_slice(
            &if self.title_listing {
                u64::MAX
            } else {
                title_ptr_pos
            }
            .to_le_bytes(),
        );
        h.extend_from_slice(&cluster_ptr_pos.to_le_bytes());
        h.extend_from_slice(&mime_list_pos.to_le_bytes());
        h.extend_from_slice(&main_page.to_le_bytes());
        h.extend_from_slice(&u32::MAX.to_le_bytes());
        h.extend_from_slice(&checksum_pos.to_le_bytes());
        out[..80].copy_from_slice(&h);
        out
    }
}

fn cluster_body(blobs: &[Vec<u8>], extended: bool) -> Vec<u8> {
    let size = if extended { 8 } else { 4 };
    let mut body = Vec::new();
    let mut off = ((blobs.len() + 1) * size) as u64;
    let push = |body: &mut Vec<u8>, v: u64| {
        if extended {
            body.extend_from_slice(&v.to_le_bytes());
        } else {
            body.extend_from_slice(&(v as u32).to_le_bytes());
        }
    };
    push(&mut body, off);
    for b in blobs {
        off += b.len() as u64;
        push(&mut body, off);
    }
    for b in blobs {
        body.extend_from_slice(b);
    }
    body
}

/// The archive most tests run against: three entries, a redirect, and metadata.
pub fn sample() -> Builder {
    Builder::new()
        .mimes(["text/html", "image/png", "text/plain"])
        .content(
            "index.html",
            "Main Page",
            0,
            b"<html><body>index</body></html>",
        )
        .content("logo.png", "Logo", 1, &[0x89, b'P', b'N', b'G', 0, 1, 2, 3])
        .content("notes.txt", "Notes", 2, b"plain notes")
        .redirect("home.html", "Home", "index.html")
        .content_in(b'M', "Title", "", 2, b"Sample Archive")
        .content_in(b'M', "Description", "", 2, b"An archive crafted for tests")
        .content_in(
            b'M',
            "Illustration_48x48@1",
            "",
            1,
            &[0x89, b'P', b'N', b'G', 0, 0xff],
        )
        .main_page("index.html")
}

/// A directory that removes itself when dropped.
#[derive(Debug)]
pub struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    /// Create a uniquely named directory under the system temporary directory.
    pub fn new(tag: &str) -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("cairn-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    /// The directory path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Write a file into the directory and return its path.
    pub fn write(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = self.path.join(name);
        std::fs::write(&p, bytes).expect("write temp file");
        p
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
