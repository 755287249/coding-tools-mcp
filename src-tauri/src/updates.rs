use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all="camelCase")]
pub struct Release {
    pub current_version: String,
    pub version: String,
    pub available: bool,
    pub page: String,
    pub notes: String,
    pub asset_url: Option<String>,
    pub sha256: Option<String>,
}
pub fn repository(value: &str) -> Result<&str,String> {
    let value=value.trim();let parts:Vec<_>=value.split('/').collect();
    if parts.len()!=2 || parts.iter().any(|part|part.is_empty() || *part=="." || *part==".." || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))) {return Err("Use a GitHub owner/repository name".into());}
    Ok(value)
}
fn version(value:&str)->Option<[u64;3]> {
    let parts=value.strip_prefix("client-v").unwrap_or(value).trim_start_matches('v').split('.').map(str::parse).collect::<Result<Vec<u64>,_>>().ok()?;
    parts.try_into().ok()
}
pub fn parse_release(repo:&str,data:Value)->Result<Release,String> {
    repository(repo)?;
    let tag=data["tag_name"].as_str().ok_or("Release tag missing")?;
    let next=version(tag).ok_or("Release must have a stable vMAJOR.MINOR.PATCH tag")?;
    let v=tag.strip_prefix("client-v").unwrap_or(tag).trim_start_matches('v').to_string();
    let prefix=format!("https://github.com/{repo}/releases/download/");
    let expected=format!("ctmcp-{v}-win64.exe");
    let asset=data["assets"].as_array().and_then(|items|items.iter().find(|a|a["name"]==expected));
    let url=asset.and_then(|a|a["browser_download_url"].as_str()).filter(|url|url.starts_with(&prefix)).map(str::to_string);
    let digest=asset.and_then(|a|a["digest"].as_str()).and_then(|d|d.strip_prefix("sha256:")).filter(|d|d.len()==64&&d.bytes().all(|b|b.is_ascii_hexdigit())).map(|v|v.to_ascii_lowercase());
    Ok(Release {current_version:env!("CARGO_PKG_VERSION").into(),version:v,available:next>version(env!("CARGO_PKG_VERSION")).unwrap_or([0;3]),page:format!("https://github.com/{repo}/releases"),notes:data["body"].as_str().unwrap_or("").chars().take(32000).collect(),asset_url:url,sha256:digest})
}
// A Worker origin or its /ctmcp base can share a domain with other applications.
fn worker_base(source:&str)->Result<reqwest::Url,String> {
    let mut url=reqwest::Url::parse(source).map_err(|_|"Use an HTTPS Worker URL or owner/repository")?;
    if url.scheme()!="https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || !matches!(url.path().trim_end_matches('/'),""|"/ctmcp") {return Err("Worker source must be an HTTPS origin or /ctmcp URL without credentials or query parameters".into());}
    url.set_path("/ctmcp/");Ok(url)
}
fn parse_worker_release(base:&reqwest::Url,data:Value)->Result<Release,String> {
    if data["appId"]!="coding-tools-mcp" {return Err("Update source is not Coding Tools MCP".into());}
    let v=data["version"].as_str().ok_or("Release version missing")?;
    let next=version(v).ok_or("Invalid stable release version")?;
    let file=&data["files"]["portable"];
    if file["name"]!=format!("ctmcp-{v}-win64.exe") || !file["size"].as_u64().is_some_and(|n|n>0&&n<=200*1024*1024) {return Err("Worker release does not contain the matching Coding Tools Windows EXE".into());}
    let digest=file["sha256"].as_str().filter(|s|s.len()==64&&s.bytes().all(|b|b.is_ascii_hexdigit())).ok_or("Worker release SHA-256 missing")?.to_ascii_lowercase();
    let mut download=base.join("download").map_err(|e|e.to_string())?;
    download.query_pairs_mut().append_pair("version",v).append_pair("sha256",&digest);
    Ok(Release{current_version:env!("CARGO_PKG_VERSION").into(),version:v.into(),available:next>version(env!("CARGO_PKG_VERSION")).unwrap_or([0;3]),page:base.join("latest").unwrap().to_string(),notes:data["notes"].as_str().unwrap_or("").chars().take(32000).collect(),asset_url:Some(download.to_string()),sha256:Some(digest)})
}
async fn metadata(mut response:reqwest::Response)->Result<Value,String>{
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await.map_err(|e|e.to_string())?{if bytes.len()+chunk.len()>2*1024*1024{return Err("Release metadata is too large".into());}bytes.extend_from_slice(&chunk);}
    serde_json::from_slice(&bytes).map_err(|_|"Invalid update metadata".into())
}
fn client()->Result<reqwest::Client,String> {
    reqwest::Client::builder().user_agent(concat!("CodingToolsMCP/",env!("CARGO_PKG_VERSION"))).timeout(std::time::Duration::from_secs(180)).build().map_err(|e|e.to_string())
}
#[cfg_attr(feature="desktop",tauri::command)]
pub async fn check_app_update(repo:String)->Result<Release,String> {
    let source=repo.trim();
    let worker=if source.contains("://"){Some(worker_base(source)?)}else{None};
    let url=if let Some(base)=&worker{base.join("latest").map_err(|e|e.to_string())?.to_string()}else{format!("https://api.github.com/repos/{}/releases/latest",repository(source)?)};
    let response=client()?.get(url).timeout(std::time::Duration::from_secs(20)).send().await.map_err(|e|e.to_string())?;
    if response.status()==reqwest::StatusCode::NOT_FOUND{return Err("No update release found. Check the update source and publish a stable Coding Tools release first.".into());}
    let response=response.error_for_status().map_err(|e|e.to_string())?;
    let data=metadata(response).await?;
    if let Some(base)=worker{parse_worker_release(&base,data)}else{parse_release(source,data)}
}
// Bind both download and installation to the release the user reviewed.
fn verify_selection(version:&str,digest:&str,expected_version:&str,expected_sha256:&str)->Result<(),String> {
    if version!=expected_version || digest!=expected_sha256 || digest.len()!=64 || !digest.bytes().all(|b|b.is_ascii_hexdigit()) {
        return Err("The release changed. Check updates again and review it before downloading or installing.".into());
    }
    Ok(())
}
#[derive(Clone)]
struct Staged {path:PathBuf,digest:String,version:String}
static STAGED:OnceLock<Mutex<Option<Staged>>>=OnceLock::new();
fn staged()->&'static Mutex<Option<Staged>>{STAGED.get_or_init(||Mutex::new(None))}
#[cfg_attr(feature="desktop",tauri::command)]
pub async fn download_app_update(repo:String,expected_version:String,expected_sha256:String)->Result<Value,String> {
    if !cfg!(all(windows,target_arch="x86_64")){return Err("Automatic installation currently supports Windows x64; use the release page for this platform.".into());}
    let release=check_app_update(repo).await?;
    verify_selection(&release.version,release.sha256.as_deref().unwrap_or(""),&expected_version,&expected_sha256)?;
    if !release.available{return Err("Already on this version or newer".into());}
    let url=release.asset_url.ok_or("Release does not contain the Windows portable EXE")?;
    let digest=release.sha256.ok_or("GitHub asset SHA-256 is missing; automatic installation is unavailable")?;
    let mut response=client()?.get(url).send().await.map_err(|e|e.to_string())?.error_for_status().map_err(|e|e.to_string())?;
    const LIMIT:usize=200*1024*1024;
    let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await.map_err(|e|e.to_string())? {if bytes.len()+chunk.len()>LIMIT{return Err("Update exceeds 200 MiB".into());}bytes.extend_from_slice(&chunk);}
    if !bytes.starts_with(b"MZ") || format!("{:x}",Sha256::digest(&bytes))!=digest {return Err("Update checksum or executable header mismatch".into());}
    let dir=crate::platform::platform().app_config_dir().map_err(|e|e.to_string())?.join("updates");
    std::fs::create_dir_all(&dir).map_err(|e|e.to_string())?;
    let path=dir.join(format!("update-{}.exe",uuid::Uuid::new_v4()));
    std::fs::write(&path,&bytes).map_err(|e|e.to_string())?;
    let previous=staged().lock().map_err(|_|"Update lock unavailable")?.replace(Staged{path,digest:digest.clone(),version:release.version.clone()});
    if let Some(old)=previous{let _=std::fs::remove_file(old.path);}
    Ok(serde_json::json!({"version":release.version,"sha256":digest,"bytes":bytes.len()}))
}
#[cfg(feature="desktop")]
#[tauri::command]
pub fn install_app_update(app:tauri::AppHandle,expected_version:String,expected_sha256:String)->Result<(),String> {
    #[cfg(not(windows))]
    {let _=(app,expected_version,expected_sha256); Err("Automatic installation requires Windows".into())}
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut staging=staged().lock().map_err(|_|"Update lock unavailable")?;
        let item=staging.as_ref().ok_or("Download and verify an update first")?;
        verify_selection(&item.version,&item.digest,&expected_version,&expected_sha256)?;
        let bytes=std::fs::read(&item.path).map_err(|e|e.to_string())?;
        if format!("{:x}",Sha256::digest(bytes))!=item.digest{return Err("Staged update checksum changed".into());}
        let target=std::env::current_exe().map_err(|e|e.to_string())?;
        let quote=|p:&std::path::Path|format!("'{}'",p.display().to_string().replace('\'',"''"));
        let script=item.path.with_extension("ps1");
        let body=include_str!("../../scripts/install-app-update.ps1")
            .replace("__TARGET__",&quote(&target)).replace("__SOURCE__",&quote(&item.path))
            .replace("__HASH__",&format!("'{}'",item.digest)).replace("__PID__",&std::process::id().to_string());
        std::fs::write(&script,body).map_err(|e|e.to_string())?;
        // A visible console gives the user update progress and any rollback error.
        std::process::Command::new("cmd.exe").args(["/k","powershell.exe","-NoProfile","-ExecutionPolicy","Bypass","-File"]).arg(&script).creation_flags(0x00000010).spawn().map_err(|e|e.to_string())?;
        // The installer owns this file now; later downloads must not remove it.
        staging.take();
        app.exit(0);Ok(())
    }
}
#[cfg(test)]
mod tests{
    use super::*;use serde_json::json;
    #[test]fn worker_routes_cannot_select_another_application(){
        let base=worker_base("https://updates.example/ctmcp").unwrap();
        let valid=json!({"appId":"coding-tools-mcp","version":"999.1.2","notes":"notes","files":{"portable":{"name":"ctmcp-999.1.2-win64.exe","size":1234,"sha256":"a".repeat(64)}}});
        let r=parse_worker_release(&base,valid.clone()).unwrap();assert!(r.available);assert!(r.asset_url.unwrap().starts_with("https://updates.example/ctmcp/download?version=999.1.2&sha256="));
        for field in ["appId","version"]{let mut wrong=valid.clone();wrong[field]=json!("brana");assert!(parse_worker_release(&base,wrong).is_err());}
        for (field,value) in [("name",json!("BranaAi-Setup-999.1.2.exe")),("sha256",json!("")),("size",json!(0)),("size",json!(210*1024*1024))]{let mut wrong=valid.clone();wrong["files"]["portable"][field]=value;assert!(parse_worker_release(&base,wrong).is_err());}
        for source in ["http://updates.example","https://user:pass@updates.example","https://updates.example/brana","https://updates.example/?token=x","https://updates.example/#x"]{assert!(worker_base(source).is_err());}
        assert_eq!(worker_base("https://updates.example/").unwrap().as_str(),"https://updates.example/ctmcp/");
    }
    #[test]fn release_selection_rejects_wrong_assets_and_downgrades(){
        let d=json!({"tag_name":"v999.1.0","body":"release","assets":[{"name":"ctmcp-999.1.0-win64.exe","browser_download_url":"https://evil.example/app.exe","digest":format!("sha256:{}","a".repeat(64))}]});
        let r=parse_release("owner/repo",d).unwrap();assert!(r.available);assert!(r.asset_url.is_none());
        assert!(!parse_release("owner/repo",json!({"tag_name":"v0.0.1","assets":[]})).unwrap().available);
        assert!(parse_release("owner/repo",json!({"tag_name":"v1.2.3-beta"})).is_err());
    }
    #[test]fn reviewed_release_must_match_download_and_staging(){
        let a="a".repeat(64);let b="b".repeat(64);
        assert!(verify_selection("1.2.3",&a,"1.2.3",&a).is_ok());
        assert!(verify_selection("1.2.4",&a,"1.2.3",&a).is_err());
        assert!(verify_selection("1.2.3",&b,"1.2.3",&a).is_err());
        assert!(verify_selection("1.2.3","","1.2.3","").is_err());
        assert!(verify_selection("1.2.3",&"x".repeat(64),"1.2.3",&"x".repeat(64)).is_err());
    }
    #[test]fn validates_repository_and_digest(){
        for repo in ["https://evil/a","a/b/c","a/..","a/","a/b?x"]{assert!(repository(repo).is_err());}
        let r=parse_release("o/r",json!({"tag_name":"v999.2.3","assets":[{"name":"ctmcp-999.2.3-win64.exe","browser_download_url":"https://github.com/o/r/releases/download/v999.2.3/ctmcp-999.2.3-win64.exe","digest":"sha256:bad"}]})).unwrap();assert!(r.asset_url.is_some());assert!(r.sha256.is_none());
    }
}
