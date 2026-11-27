//! Session-local table aliases backed by Lance Namespace and its DataFusion adapter.
//! No catalog files or source datasets are modified by registration/removal.
use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use datafusion::{
    catalog::{SchemaProvider, memory::MemoryCatalogProvider},
    dataframe::DataFrame,
    datasource::TableProvider,
    prelude::{SQLOptions, SessionConfig, SessionContext},
};
use lance::{Dataset, datafusion::LanceTableProvider};
use lance_namespace::{
    LanceNamespace, NamespaceError,
    models::{DescribeTableRequest, DescribeTableResponse},
};
use lance_namespace_datafusion::{LanceSchemaProvider, NamespaceLevel};
use serde::{Deserialize, Serialize};
use tokio::sync::{OnceCell, RwLock};
use uuid::Uuid;

use super::{
    error::{ApiError, RequestError::InvalidRequest},
    state::{AppState, ConnectedDataset, ConnectionQuery, connected},
    validation::validate_name,
};

#[derive(Clone)]
pub(super) struct Source {
    uri: String,
    info: TableInfo,
}

// Dataset URIs may contain credentials; avoid including them in debug output.
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("name", &self.info.name)
            .finish_non_exhaustive()
    }
}

pub(super) type SessionTables = Arc<RwLock<BTreeMap<String, Source>>>;

#[derive(Clone, Debug, Serialize)]
struct TableInfo {
    name: String,
    uri: String,
    validated_version: u64,
    columns: Vec<String>,
}

#[derive(Deserialize)]
struct AttachRequest {
    name: String,
    uri: String,
}

pub(crate) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sql/tables", get(list).post(attach))
        .route("/sql/tables/{name}", axum::routing::delete(detach))
}

async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConnectionQuery>,
) -> Result<Json<Vec<TableInfo>>, ApiError> {
    let connection = connected(&state, query.connection_id)?;
    let tables = connection.sql_tables.read().await;
    Ok(Json(
        tables.values().map(|source| source.info.clone()).collect(),
    ))
}

fn invalid(error: impl std::fmt::Display) -> ApiError {
    ApiError(anyhow!(InvalidRequest(error.to_string())))
}

async fn attach(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConnectionQuery>,
    Json(request): Json<AttachRequest>,
) -> Result<Json<TableInfo>, ApiError> {
    let connection = connected(&state, query.connection_id)?;
    validate_name(&request.name).map_err(invalid)?;
    if request.uri.trim().is_empty() {
        return Err(invalid("Dataset location cannot be empty"));
    }
    let uri = request.uri.trim().to_string();
    let dataset = Dataset::open(&uri).await.map_err(invalid)?;
    let info = TableInfo {
        name: request.name.clone(),
        uri: request.uri.trim().to_string(),
        validated_version: dataset.version().version,
        columns: dataset
            .schema()
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect(),
    };
    insert_source(
        &connection.sql_tables,
        Source {
            uri,
            info: info.clone(),
        },
    )
    .await
    .map_err(invalid)?;
    Ok(Json(info))
}

async fn insert_source(tables: &SessionTables, source: Source) -> Result<()> {
    let mut tables = tables.write().await;
    if tables.contains_key(&source.info.name) {
        bail!("Table {} is already registered", source.info.name);
    }
    tables.insert(source.info.name.clone(), source);
    Ok(())
}

async fn detach(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConnectionQuery>,
    Path(name): Path<String>,
) -> Result<axum::http::StatusCode, ApiError> {
    let connection = connected(&state, query.connection_id)?;
    if connection.sql_tables.write().await.remove(&name).is_none() {
        return Err(invalid("Unknown registered table"));
    }
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Immutable catalog view for one query.
#[derive(Debug)]
struct QueryNamespace {
    sources: BTreeMap<String, Source>,
    id: Uuid,
}

#[async_trait]
impl LanceNamespace for QueryNamespace {
    fn namespace_id(&self) -> String {
        format!("viewer-query-{}", self.id)
    }
    async fn describe_table(
        &self,
        request: DescribeTableRequest,
    ) -> lance_core::Result<DescribeTableResponse> {
        let id = request.id.unwrap_or_default();
        let source = if id.len() == 1 {
            self.sources.get(&id[0])
        } else {
            None
        };
        let source = source.ok_or_else(|| {
            lance_core::Error::from(NamespaceError::TableNotFound {
                message: id.join("."),
            })
        })?;
        Ok(DescribeTableResponse {
            location: Some(source.uri.clone()),
            ..Default::default()
        })
    }
}

/// The upstream schema provider refreshes Latest on each lookup. Cache each
/// lookup once per query, including repeated/self-join references, to pin inputs.
#[derive(Debug)]
struct QuerySchema {
    inner: LanceSchemaProvider,
    tables: BTreeMap<String, OnceCell<Arc<dyn TableProvider>>>,
    selected: Arc<dyn TableProvider>,
}

#[async_trait]
impl SchemaProvider for QuerySchema {
    fn table_names(&self) -> Vec<String> {
        std::iter::once("dataset".to_string())
            .chain(self.tables.keys().cloned())
            .collect()
    }
    fn table_exist(&self, name: &str) -> bool {
        name == "dataset" || self.tables.contains_key(name)
    }
    async fn table(
        &self,
        name: &str,
    ) -> datafusion::common::Result<Option<Arc<dyn TableProvider>>> {
        if name == "dataset" {
            return Ok(Some(self.selected.clone()));
        }
        let Some(slot) = self.tables.get(name) else {
            return Ok(None);
        };
        let provider = slot
            .get_or_try_init(|| async {
                self.inner.table(name).await?.ok_or_else(|| {
                    datafusion::common::DataFusionError::Plan(format!("Unknown table {name}"))
                })
            })
            .await?;
        Ok(Some(provider.clone()))
    }
}

pub(super) async fn prepare(
    connection: &ConnectedDataset,
    sql: &str,
    batch_size: usize,
) -> Result<DataFrame> {
    let sources = connection.sql_tables.read().await.clone();
    let tables = sources
        .keys()
        .map(|name| (name.clone(), OnceCell::new()))
        .collect();
    let namespace = Arc::new(QueryNamespace {
        sources,
        id: Uuid::new_v4(),
    });
    let namespace_client: Arc<dyn LanceNamespace> = namespace.clone();
    let schema = Arc::new(QuerySchema {
        inner: LanceSchemaProvider::try_new(NamespaceLevel::from_root(namespace_client)).await?,
        tables,
        selected: Arc::new(LanceTableProvider::new(
            connection.dataset.clone(),
            false,
            false,
        )),
    });
    let catalog = Arc::new(MemoryCatalogProvider::new());
    use datafusion::catalog::CatalogProvider;
    catalog.register_schema("public", schema)?;
    let ctx = SessionContext::new_with_config(
        SessionConfig::new()
            .with_batch_size(batch_size)
            .with_default_catalog_and_schema("viewer", "public"),
    );
    ctx.register_catalog("viewer", catalog);
    let options = SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(false)
        .with_allow_statements(false);
    Ok(ctx.sql_with_options(sql, options).await?)
}

#[cfg(test)]
pub(super) async fn attach_test(
    connection: &ConnectedDataset,
    name: &str,
    dataset: &lance::Dataset,
) -> Result<()> {
    insert_source(
        &connection.sql_tables,
        Source {
            uri: dataset.uri().to_string(),
            info: TableInfo {
                name: name.into(),
                uri: dataset.uri().to_string(),
                validated_version: dataset.version().version,
                columns: vec![],
            },
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registration_rejects_duplicates_without_a_table_count_limit() {
        let tables = SessionTables::default();
        let source = |name: String| Source {
            uri: "unused".into(),
            info: TableInfo {
                name,
                uri: "unused".into(),
                validated_version: 1,
                columns: vec![],
            },
        };
        let (left, right) = tokio::join!(
            insert_source(&tables, source("same".into())),
            insert_source(&tables, source("same".into()))
        );
        assert_ne!(left.is_ok(), right.is_ok());
        for i in 1..=32 {
            insert_source(&tables, source(format!("t{i}")))
                .await
                .unwrap();
        }
        assert_eq!(tables.read().await.len(), 33);
    }

    #[tokio::test]
    async fn registration_validates_sources_and_removal_preserves_data() {
        use super::super::{dataset::build_dataset_info, state::SessionEntry};
        use arrow_array::{Int32Array, RecordBatch, RecordBatchIterator};
        use arrow_schema::{DataType, Field, Schema};

        let root = tempfile::tempdir().unwrap();
        let uri = root.path().join("source.lance");
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(Int32Array::from(vec![1]))])
            .unwrap();
        let dataset = Arc::new(
            Dataset::write(
                RecordBatchIterator::new([Ok(batch)], schema),
                uri.to_str().unwrap(),
                None,
            )
            .await
            .unwrap(),
        );
        let info = Arc::new(
            build_dataset_info(&dataset, dataset.uri(), "main")
                .await
                .unwrap(),
        );
        let connection = ConnectedDataset::new(dataset.clone(), info);
        let state = Arc::new(AppState::new());
        let connection_id = Uuid::new_v4();
        state
            .connections
            .insert(connection_id, SessionEntry::new(connection));
        let register = |name: &str, uri: String| {
            attach(
                State(state.clone()),
                Query(ConnectionQuery { connection_id }),
                Json(AttachRequest {
                    name: name.into(),
                    uri,
                }),
            )
        };
        assert!(register("dataset", dataset.uri().into()).await.is_err());
        assert!(
            register(
                "missing",
                root.path().join("missing.lance").display().to_string()
            )
            .await
            .is_err()
        );
        assert!(
            list(
                State(state.clone()),
                Query(ConnectionQuery { connection_id })
            )
            .await
            .map_err(|error| error.0)
            .unwrap()
            .0
            .is_empty()
        );
        let registered = register("recordings", format!("  {}  ", dataset.uri()))
            .await
            .map_err(|error| error.0)
            .unwrap()
            .0;
        assert_eq!(registered.name, "recordings");
        assert_eq!(registered.columns, ["id"]);
        assert_eq!(registered.validated_version, dataset.version().version);
        assert!(register("recordings", dataset.uri().into()).await.is_err());
        assert_eq!(
            list(
                State(state.clone()),
                Query(ConnectionQuery { connection_id })
            )
            .await
            .map_err(|error| error.0)
            .unwrap()
            .0
            .len(),
            1
        );
        detach(
            State(state.clone()),
            Query(ConnectionQuery { connection_id }),
            Path("recordings".into()),
        )
        .await
        .map_err(|error| error.0)
        .unwrap();
        assert!(
            list(
                State(state.clone()),
                Query(ConnectionQuery { connection_id })
            )
            .await
            .map_err(|error| error.0)
            .unwrap()
            .0
            .is_empty()
        );
        assert!(
            detach(
                State(state),
                Query(ConnectionQuery { connection_id }),
                Path("recordings".into())
            )
            .await
            .is_err()
        );
        assert_eq!(
            Dataset::open(dataset.uri())
                .await
                .unwrap()
                .count_rows(None)
                .await
                .unwrap(),
            1
        );
    }
}
