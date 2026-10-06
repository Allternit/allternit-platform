//! Owner-only Memory Drive API, mounted once behind the normal auth middleware.
use std::path::PathBuf;
use std::sync::Arc;
use axum::{extract::{DefaultBodyLimit, FromRef, Query, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::{get,post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use crate::{auth::get_user, db::DbHandle, memory_drive::{DriveError, Operation}, memory_drive_service::{self as service, ServiceError}, AppState};

#[derive(Clone)]
struct DriveRouteState { db:DbHandle, brains_dir:PathBuf }
impl FromRef<Arc<AppState>> for DriveRouteState {
    fn from_ref(state:&Arc<AppState>)->Self { Self{db:state.db.clone(),brains_dir:state.config.brains_dir()} }
}
pub fn memory_drive_router()->Router<Arc<AppState>> { routes() }
fn routes<S>()->Router<S> where S:Clone+Send+Sync+'static, DriveRouteState:FromRef<S> {
    Router::new()
        .route("/memory/drive/info",get(info))
        .route("/memory/drive/tree",get(tree))
        .route("/memory/drive/file",get(file))
        .route("/memory/drive/history",get(history))
        .route("/memory/drive/diff",get(diff))
        .route("/memory/drive/write",post(write))
        .route("/memory/drive/reindex",post(reindex))
        .route("/memory/drive/health",get(health))
        .route("/memory/drive/import",post(import))
        .layer(DefaultBodyLimit::max(3*1024*1024))
}
fn owner(headers:&HeaderMap)->std::result::Result<String,Response> {
    get_user(headers).filter(|u|!u.user_id.is_empty()).map(|u|u.user_id)
        .ok_or_else(||(StatusCode::UNAUTHORIZED,Json(json!({"error":"Sign in to access your memory drive."}))).into_response())
}
fn error(error:ServiceError)->Response {
    let (status,body)=match &error {
        ServiceError::NotFound | ServiceError::Drive(DriveError::NotFound(_)) =>(StatusCode::NOT_FOUND,json!({"error":error.to_string()})),
        ServiceError::Drive(DriveError::Conflict{expected,actual}) =>(StatusCode::CONFLICT,json!({"error":error.to_string(),"expected_revision":expected,"actual_revision":actual,"retry":"Read latest snapshot and reconcile before retrying."})),
        ServiceError::IndexPending{revision}=>(StatusCode::SERVICE_UNAVAILABLE,json!({"error":error.to_string(),"committed":true,"revision":revision,"index_dirty":true})),
        ServiceError::Database(_) | ServiceError::Drive(DriveError::Io(_)|DriveError::Git{..}) =>(StatusCode::INTERNAL_SERVER_ERROR,json!({"error":"Memory storage is unavailable. Retry later."})),
        ServiceError::Drive(DriveError::OwnerMismatch)=>(StatusCode::NOT_FOUND,json!({"error":"Memory drive not found."})),
        _=>(StatusCode::BAD_REQUEST,json!({"error":error.to_string()})),
    };
    (status,Json(body)).into_response()
}
async fn run(context:DriveRouteState, headers:HeaderMap, f:impl FnOnce(DriveRouteState,String)->service::Result<Value>+Send+'static)->Response {
    let owner=match owner(&headers){Ok(owner)=>owner,Err(response)=>return response};
    match tokio::task::spawn_blocking(move||f(context,owner)).await {
        Ok(Ok(value))=>Json(value).into_response(),
        Ok(Err(err))=>error(err),
        Err(_)=>(StatusCode::INTERNAL_SERVER_ERROR,Json(json!({"error":"Memory request failed. Retry later."}))).into_response(),
    }
}
#[derive(Default,Deserialize)]
#[serde(deny_unknown_fields)]
struct RevisionQuery { revision:Option<String> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery { path:String,revision:Option<String> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryQuery { path:Option<String>,limit:Option<usize> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffQuery { from:String,to:String,path:Option<String> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteBody { expected_revision:String,operations:Vec<Operation> }
#[derive(Default,Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportBody { #[serde(default)] apply:bool,expected_revision:Option<String> }

async fn info(State(context):State<DriveRouteState>,headers:HeaderMap)->Response {
    run(context,headers,|c,owner|{
        let record=service::provision(&c.db,&c.brains_dir,&owner)?;
        let snapshot=service::read(&c.db,&owner,None)?;
        Ok(json!({"name":record.name,"brain_id":record.brain_id,"revision":snapshot.revision,"indexed_revision":record.indexed_revision,
            "clone_path":format!("/api/v1/brains/{}/git",record.brain_id),"clone_token_available":false,"note":"Repo-scoped clone credentials are not implemented in this bounded pass."}))
    }).await
}
async fn tree(State(context):State<DriveRouteState>,headers:HeaderMap,Query(query):Query<RevisionQuery>)->Response {
    run(context,headers,move|c,owner|{
        let snapshot=service::read(&c.db,&owner,query.revision.as_deref())?;
        let files:Vec<_>=snapshot.files.iter().map(|(path,content)|json!({"path":path,"bytes":content.len()})).collect();
        Ok(json!({"revision":snapshot.revision,"files":files}))
    }).await
}
async fn file(State(context):State<DriveRouteState>,headers:HeaderMap,Query(query):Query<FileQuery>)->Response {
    run(context,headers,move|c,owner|{
        crate::memory_drive::validate_path(&query.path)?;
        let mut snapshot=service::read(&c.db,&owner,query.revision.as_deref())?;
        let content=snapshot.files.remove(&query.path).ok_or_else(||DriveError::NotFound(query.path.clone()))?;
        Ok(json!({"revision":snapshot.revision,"path":query.path,"content":content}))
    }).await
}
async fn history(State(context):State<DriveRouteState>,headers:HeaderMap,Query(query):Query<HistoryQuery>)->Response {
    run(context,headers,move|c,owner|{
        service::repair_index(&c.db,&owner)?;
        let commits=service::resolve(&c.db,&owner)?.storage()?.history(query.path.as_deref(),query.limit.unwrap_or(25))?;
        Ok(json!({"commits":commits}))
    }).await
}
async fn diff(State(context):State<DriveRouteState>,headers:HeaderMap,Query(query):Query<DiffQuery>)->Response {
    run(context,headers,move|c,owner|{
        service::repair_index(&c.db,&owner)?;
        let patch=service::resolve(&c.db,&owner)?.storage()?.diff(&query.from,&query.to,query.path.as_deref())?;
        Ok(json!({"from":query.from,"to":query.to,"path":query.path,"diff":patch}))
    }).await
}
async fn write(State(context):State<DriveRouteState>,headers:HeaderMap,Json(body):Json<WriteBody>)->Response {
    run(context,headers,move|c,owner|{
        let result=service::apply(&c.db,&owner,&body.expected_revision,&body.operations,"Edit memory via Allternit")?;
        Ok(json!({"revision":result.revision,"changed":result.changed,"indexed":true}))
    }).await
}
async fn reindex(State(context):State<DriveRouteState>,headers:HeaderMap)->Response {
    run(context,headers,|c,owner|{
        let drive=service::reindex(&c.db,&owner)?;
        Ok(json!({"indexed_revision":drive.indexed_revision,"index_dirty":drive.dirty_revision.is_some()}))
    }).await
}
async fn health(State(context):State<DriveRouteState>,headers:HeaderMap)->Response {
    run(context,headers,|c,owner|{
        service::repair_index(&c.db,&owner)?;
        let drive=service::resolve(&c.db,&owner)?;
        Ok(json!({"revision":drive.storage()?.head()?,"indexed_revision":drive.indexed_revision,"index_dirty":drive.dirty_revision.is_some(),"imported_at":drive.imported_at}))
    }).await
}
async fn import(State(context):State<DriveRouteState>,headers:HeaderMap,body:Option<Json<ImportBody>>)->Response {
    let body=body.map(|Json(b)|b).unwrap_or_default();
    run(context,headers,move|c,owner|{
        if !body.apply { return Ok(json!({"dry_run":true,"plan":service::import_plan(&c.db,&owner)?})); }
        let expected=body.expected_revision.ok_or_else(||ServiceError::Provenance("apply requires expected_revision from drive info".into()))?;
        let result=service::import_apply(&c.db,&owner,&expected)?;
        Ok(json!({"dry_run":false,"revision":result.revision,"changed":result.changed,"indexed":true}))
    }).await
}
