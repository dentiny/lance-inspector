use std::collections::HashSet;

use anyhow::{Result, anyhow, bail};
use arrow_schema::Schema;
use datafusion::sql::sqlparser::{
    ast::{self, Statement, TableFactor, Visit, Visitor},
    dialect::GenericDialect,
    parser::Parser,
};

use super::{error::RequestError::InvalidRequest, schema::contains_blob};

pub(super) fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    if !matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        || name.len() > 63
        || name == "dataset"
    {
        bail!(
            "Use a lowercase SQL table name (letters, digits, underscores, max 63 characters); dataset is reserved"
        );
    }
    Ok(())
}

pub(super) fn read_only_sql(sql: &str) -> Result<&str> {
    let sql = sql.trim().trim_end_matches(';').trim_end();
    if sql.is_empty() {
        return Err(anyhow!(InvalidRequest(
            "SQL query cannot be empty".to_string()
        )));
    }
    validate_sql(sql).map_err(|e| anyhow!(InvalidRequest(e.to_string())))?;
    Ok(sql)
}

/// Validate one read-only statement and detect named sources beyond `dataset`.
/// Table functions are deliberately unavailable; sources are registered via UI.
pub(super) fn validate_sql(sql: &str) -> Result<bool> {
    use std::ops::ControlFlow;
    struct Policy {
        named: bool,
        ctes: Vec<Vec<String>>,
    }
    impl Visitor for Policy {
        type Break = anyhow::Error;
        fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
            if !matches!(statement, Statement::Query(_)) {
                return ControlFlow::Break(anyhow!("Only read-only queries are supported"));
            }
            ControlFlow::Continue(())
        }
        fn pre_visit_query(&mut self, query: &ast::Query) -> ControlFlow<Self::Break> {
            self.ctes.push(
                query
                    .with
                    .as_ref()
                    .map(|with| {
                        with.cte_tables
                            .iter()
                            .map(|cte| cte.alias.name.to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
            );
            ControlFlow::Continue(())
        }
        fn post_visit_query(&mut self, _query: &ast::Query) -> ControlFlow<Self::Break> {
            self.ctes.pop();
            ControlFlow::Continue(())
        }
        fn pre_visit_select(&mut self, select: &ast::Select) -> ControlFlow<Self::Break> {
            if select.into.is_some() {
                return ControlFlow::Break(anyhow!("SELECT INTO is not supported"));
            }
            ControlFlow::Continue(())
        }
        fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<Self::Break> {
            match factor {
                TableFactor::Table { name, args, .. } => {
                    if args.is_some() {
                        return ControlFlow::Break(anyhow!(
                            "Register datasets as named tables; table functions are not supported"
                        ));
                    }
                    let name = name.to_string();
                    let is_selected = name.eq_ignore_ascii_case("dataset") || name == "\"dataset\"";
                    let is_cte = self.ctes.iter().flatten().any(|cte| cte == &name);
                    self.named |= !is_selected && !is_cte;
                }
                TableFactor::Derived { .. } => {}
                _ => return ControlFlow::Break(anyhow!("Unsupported table source")),
            }
            ControlFlow::Continue(())
        }
    }
    let statements = Parser::parse_sql(&GenericDialect {}, sql)?;
    if statements.len() != 1 {
        bail!("Exactly one read-only query is required");
    }
    let mut policy = Policy {
        named: false,
        ctes: vec![],
    };
    if let ControlFlow::Break(error) = statements.visit(&mut policy) {
        return Err(error);
    }
    Ok(policy.named)
}

pub(super) fn validate_result_schema(result_schema: &Schema, named: bool) -> Result<()> {
    if named
        && result_schema
            .fields()
            .iter()
            .any(|field| contains_blob(field))
    {
        return Err(anyhow!(InvalidRequest("Named-table queries support metadata results; use dataset alone for Lance Blob previews".into())));
    }
    let mut names = HashSet::new();
    if result_schema
        .fields()
        .iter()
        .any(|field| !names.insert(field.name()))
    {
        return Err(anyhow!(InvalidRequest(
            "SQL output names must be unique; use AS aliases".into()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_writes_and_unregistered_source_functions() {
        for sql in [
            "DELETE FROM dataset",
            "SELECT 1; SELECT 2",
            "SELECT * INTO x FROM dataset",
            "COPY (SELECT 1) TO '/tmp/x'",
            "WITH x AS (SELECT 1) DELETE FROM dataset",
            "SELECT * FROM read_parquet('/tmp/x')",
            "SELECT * FROM read_dataset('x')",
            "",
        ] {
            assert!(validate_sql(sql).is_err(), "accepted {sql}");
        }
        assert!(!validate_sql("-- comment\n SELECT * FROM dataset").unwrap());
        assert!(!validate_sql("WITH x AS (SELECT * FROM dataset) SELECT * FROM x").unwrap());
        assert!(validate_sql("WITH x AS (SELECT * FROM recordings) SELECT * FROM x").unwrap());
        assert!(validate_sql("SELECT * FROM dataset d JOIN recordings r ON d.id=r.id").unwrap());
    }

    #[test]
    fn validates_aliases_and_reserves_dataset() {
        for name in ["dataset", "", "X", "1x", "a.b", "a-b", "a b"] {
            assert!(validate_name(name).is_err());
        }
        for name in ["recordings", "_base", "segments_2"] {
            validate_name(name).unwrap();
        }
    }

    #[test]
    fn accepts_only_read_only_sql() {
        assert_eq!(
            read_only_sql(" SELECT * FROM dataset; ").unwrap(),
            "SELECT * FROM dataset"
        );
        assert!(read_only_sql("WITH selected AS (SELECT 1) SELECT * FROM selected").is_ok());
        assert!(read_only_sql("DELETE FROM dataset").is_err());
        assert!(read_only_sql("CREATE EXTERNAL TABLE secret").is_err());
        assert!(read_only_sql("  ").is_err());
    }

    #[test]
    fn named_results_reject_nested_blobs_but_allow_ordinary_binary() {
        use arrow_schema::{DataType, Field};
        use std::sync::Arc;
        let blob = lance::blob_field("payload", true);
        let nested = Field::new(
            "nested",
            DataType::List(Arc::new(Field::new(
                "item",
                DataType::Struct(vec![blob].into()),
                true,
            ))),
            true,
        );
        assert!(validate_result_schema(&Schema::new(vec![nested]), true).is_err());
        let binary = Schema::new(vec![Field::new("bytes", DataType::Binary, true)]);
        validate_result_schema(&binary, true).unwrap();
    }
}
