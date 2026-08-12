use arrow_schema::{Field, Schema as ArrowSchema};

use crate::models::MediaColumn;

// Arrow extension name identifying Lance Blob V2 fields.
const BLOB_EXTENSION: &str = "lance.blob.v2";

pub(super) struct DatasetColumns {
    pub(super) scalar: Vec<String>,
    pub(super) media: Vec<MediaColumn>,
}

pub(super) struct SqlResultColumns {
    pub(super) scalar_indices: Vec<usize>,
    pub(super) scalar: Vec<String>,
    pub(super) media: Vec<MediaColumn>,
}

pub(super) fn dataset_columns(schema: &ArrowSchema) -> DatasetColumns {
    let mut scalar = Vec::new();
    let mut media = Vec::new();
    for field in schema.fields() {
        if is_blob_field(field) {
            media.push(media_column(field, schema));
        } else {
            scalar.push(field.name().clone());
        }
    }
    DatasetColumns { scalar, media }
}

pub(super) fn sql_result_columns(
    result_schema: &ArrowSchema,
    dataset_schema: &ArrowSchema,
) -> SqlResultColumns {
    let mut scalar_indices = Vec::new();
    let mut scalar = Vec::new();
    let mut media = Vec::new();
    for (index, field) in result_schema.fields().iter().enumerate() {
        if is_sql_blob_field(field, dataset_schema) {
            media.push(media_column(field, result_schema));
        } else {
            scalar_indices.push(index);
            scalar.push(field.name().clone());
        }
    }
    SqlResultColumns {
        scalar_indices,
        scalar,
        media,
    }
}

fn media_column(field: &Field, schema: &ArrowSchema) -> MediaColumn {
    let mime_name = format!("{}_mime", field.name());
    MediaColumn {
        name: field.name().clone(),
        mime_column: schema.field_with_name(&mime_name).ok().map(|_| mime_name),
    }
}

fn is_sql_blob_field(field: &Field, dataset_schema: &ArrowSchema) -> bool {
    is_blob_field(field)
        || dataset_schema
            .field_with_name(field.name())
            .is_ok_and(is_blob_field)
}

pub(super) fn is_blob_field(field: &Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .is_some_and(|name| name == BLOB_EXTENSION)
}
