use crate::models::RemotePackage;
use anyhow::Result;

pub mod deb_repo;
pub mod direct_url;
pub mod github;
pub mod rpm_repo;
pub mod sourceforge;

/// Common interface for all upstream package sources.
///
/// Each source type (GitHub, Debian APT, RPM, direct URL, SourceForge)
/// provides a `fetch_latest` method with this signature.  The [`AnySource`]
/// enum dispatches to the correct implementation at runtime.
///
/// This trait documents the contract; the concrete types are dispatched via
/// [`AnySource`] rather than `dyn PackageSource` (which would require
/// boxing the returned future).
#[allow(dead_code)]
pub trait PackageSource {
    /// Fetch up to `n` newest packages from the upstream source.
    fn fetch_latest(
        &self,
        n: usize,
    ) -> impl std::future::Future<Output = Result<Vec<RemotePackage>>> + Send;
}

/// Enum-based dispatch wrapper for all source types.
///
/// This avoids the need for `dyn PackageSource` (which doesn't work with
/// `async fn` in traits) while still providing a single type that `sync.rs`
/// can work with uniformly.
pub enum AnySource {
    Github(github::GithubSource),
    DebRepo(deb_repo::DebRepoSource),
    RpmRepo(rpm_repo::RpmRepoSource),
    DirectUrl(direct_url::DirectUrlSource),
    Sourceforge(sourceforge::SourceforgeSource),
}

impl AnySource {
    /// Fetch up to `n` newest packages from the underlying source.
    pub async fn fetch_latest(&self, n: usize) -> Result<Vec<RemotePackage>> {
        match self {
            Self::Github(s) => s.fetch_latest(n).await,
            Self::DebRepo(s) => s.fetch_latest(n).await,
            Self::RpmRepo(s) => s.fetch_latest(n).await,
            Self::DirectUrl(s) => s.fetch_latest(n).await,
            Self::Sourceforge(s) => s.fetch_latest(n).await,
        }
    }
}
