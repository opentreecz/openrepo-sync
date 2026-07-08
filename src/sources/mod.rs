use crate::models::RemotePackage;
use anyhow::Result;

pub mod direct_url;
pub mod github;
pub mod sourceforge;

#[allow(dead_code)]
pub trait PackageSource {
    async fn fetch_latest(&self, n: usize) -> Result<Vec<RemotePackage>>;
}
