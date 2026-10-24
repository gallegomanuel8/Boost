//! raudo download engine: segmented HTTP downloads with dynamic splitting and resume.

pub mod engine;
pub mod hls;
pub mod http;
pub mod limiter;
pub mod model;

pub use engine::{move_file, part_location, part_path, run, unique_path, Dest, Error, Job, Outcome, PART_SUFFIX};
pub use limiter::Limiter;
pub use model::{Event, Options, Request, Segment, State};

/// HTTP client tuned for downloads (no automatic decompression, generous redirects).
pub fn client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(std::time::Duration::from_secs(20))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
}
