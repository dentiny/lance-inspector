use std::collections::HashMap;

use arrow_schema::{DataType, Field, Schema};
use lance_core::datatypes::BlobV2Layout;

use crate::models::MediaColumn;

// Arrow extension name identifying Lance Blob V2 fields.
const BLOB_EXTENSION: &str = "lance.blob.v2";

pub(super) struct DatasetColumns {
    pub(super) projection: Vec<String>,
    pub(super) scalar: Vec<String>,
    pub(super) media: Vec<MediaProjection>,
}

pub(super) struct SqlColumns {
    pub(super) scalar_indices: Vec<usize>,
    pub(super) scalar: Vec<String>,
    pub(super) media_projections: Vec<MediaProjection>,
}

#[derive(Clone)]
pub(super) struct MediaProjection {
    pub(super) result_index: usize,
    pub(super) column: MediaColumn,
}

pub(super) fn dataset_columns(
    schema: &Schema,
    source_field_ids: &HashMap<String, i32>,
) -> DatasetColumns {
    let mut projection = Vec::new();
    let mut scalar = Vec::new();
    let mut media = Vec::new();
    for (index, field) in schema.fields().iter().enumerate() {
        projection.push(field.name().clone());
        if is_blob_field(field) || is_blob_array_field(field) {
            media.push(media_projection(
                field,
                source_field_ids[field.name()],
                index,
            ));
        } else {
            scalar.push(field.name().clone());
        }
    }
    DatasetColumns {
        projection,
        scalar,
        media,
    }
}

pub(super) fn sql_columns(
    result_schema: &Schema,
    dataset_schema: &Schema,
    source_names: &[Option<String>],
    source_field_ids: &HashMap<String, i32>,
) -> SqlColumns {
    let mut scalar_indices = Vec::new();
    let mut scalar = Vec::new();
    let mut media_projections = Vec::new();
    for (index, field) in result_schema.fields().iter().enumerate() {
        let source_name = source_names
            .get(index)
            .and_then(Option::as_deref)
            .unwrap_or(field.name());
        let source_field = dataset_schema.field_with_name(source_name).ok();
        let array = is_blob_array_field(field) && source_field.is_some_and(is_blob_array_field);
        let scalar_blob = is_blob_field(field) && source_field.is_some_and(is_blob_field);
        if array || scalar_blob {
            media_projections.push(media_projection(
                field,
                source_field_ids[source_name],
                index,
            ));
            continue;
        }
        scalar_indices.push(index);
        scalar.push(field.name().clone());
    }
    SqlColumns {
        scalar_indices,
        scalar,
        media_projections,
    }
}

pub(super) fn is_blob_field(field: &Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .is_some_and(|name| name == BLOB_EXTENSION)
        || match field.data_type() {
            DataType::Struct(fields) => BlobV2Layout::classify(fields).is_some(),
            _ => false,
        }
}

pub(super) fn is_blob_array_field(field: &Field) -> bool {
    match field.data_type() {
        DataType::List(item) | DataType::LargeList(item) | DataType::FixedSizeList(item, _) => {
            is_blob_field(item) || is_blob_array_field(item)
        }
        _ => false,
    }
}

fn media_projection(field: &Field, source_field_id: i32, result_index: usize) -> MediaProjection {
    MediaProjection {
        result_index,
        column: MediaColumn {
            name: field.name().clone(),
            source_field_id,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use lance_core::datatypes::BLOB_V2_DESC_FIELDS;

    use super::*;

    #[test]
    fn recognizes_sql_blob_array_from_dataset_schema() {
        let blob =
            Field::new("item", DataType::LargeBinary, true).with_metadata(HashMap::from([(
                "ARROW:extension:name".to_string(),
                BLOB_EXTENSION.to_string(),
            )]));
        let dataset_schema = Schema::new(vec![Field::new(
            "image_array",
            DataType::List(Arc::new(blob)),
            true,
        )]);
        let descriptor = Field::new("item", DataType::Struct(BLOB_V2_DESC_FIELDS.clone()), true);
        let result_schema = Schema::new(vec![Field::new(
            "image_array",
            DataType::List(Arc::new(descriptor)),
            true,
        )]);

        let columns = sql_columns(
            &result_schema,
            &dataset_schema,
            &[Some("image_array".to_string())],
            &HashMap::from([("image_array".to_string(), 7)]),
        );

        assert_eq!(columns.media_projections.len(), 1);
        assert_eq!(columns.media_projections[0].column.source_field_id, 7);
    }

    #[test]
    fn ignores_non_blob_alias_collisions() {
        let blob =
            Field::new("blob", DataType::LargeBinary, true).with_metadata(HashMap::from([(
                "ARROW:extension:name".to_string(),
                BLOB_EXTENSION.to_string(),
            )]));
        let result = Schema::new(vec![Field::new("blob", DataType::Int32, false)]);

        let columns = sql_columns(
            &result,
            &Schema::new(vec![blob]),
            &[None],
            &HashMap::from([("blob".to_string(), 3)]),
        );

        assert!(columns.media_projections.is_empty());
        assert_eq!(columns.scalar, vec!["blob"]);
    }

    #[test]
    fn recognizes_nested_blob_arrays() {
        let descriptor = Field::new("item", DataType::Struct(BLOB_V2_DESC_FIELDS.clone()), true);
        let nested = Field::new(
            "frames",
            DataType::List(Arc::new(Field::new(
                "items",
                DataType::LargeList(Arc::new(descriptor)),
                true,
            ))),
            true,
        );

        assert!(is_blob_array_field(&nested));
    }
}
