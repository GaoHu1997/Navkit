//! Thin SQLx wrappers: same query text, arguments, executor, streaming and cancellation semantics.
use super::{ExecutorContext, Guard, Meta};

/// A describe sends the original SQL for preparation/metadata, not execution. Do not hide this distinction.
pub fn describe<'e, 'q: 'e, 'c: 'e, E>(
    executor: E,
    sql: &'q str,
) -> futures::future::BoxFuture<'e, Result<sqlx::Describe<E::Database>, sqlx::Error>>
where
    E: sqlx::Executor<'c> + ExecutorContext + 'e,
{
    let mut context = executor.sql_context();
    context.origin = "metadata".into();
    Box::pin(async move {
        let mut guard = Guard::in_context(sql, Vec::new(), context);
        let result = executor.describe(sql).await;
        match &result {
            Ok(_) => guard.finish(
                true,
                -1,
                "SQL prepared/described for metadata; not executed".into(),
            ),
            Err(error) => guard.finish(false, -1, error.to_string()),
        }
        result
    })
}

use futures::{stream::BoxStream, Stream};
use sqlx::{Database, Encode, Executor, FromRow, IntoArguments, Type};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

pub trait Affected {
    fn affected(&self) -> u64;
}
impl Affected for sqlx::sqlite::SqliteQueryResult {
    fn affected(&self) -> u64 {
        self.rows_affected()
    }
}
impl Affected for sqlx::mysql::MySqlQueryResult {
    fn affected(&self) -> u64 {
        self.rows_affected()
    }
}
impl Affected for sqlx::postgres::PgQueryResult {
    fn affected(&self) -> u64 {
        self.rows_affected()
    }
}
struct LoggedStream<'e, T> {
    inner: BoxStream<'e, Result<T, sqlx::Error>>,
    sql: String,
    meta: Meta,
    context: super::Context,
    guard: Option<Guard>,
    rows: i64,
    affected: i64,
    count: fn(&T) -> i64,
    done: bool,
}
impl<T> Stream for LoggedStream<'_, T> {
    type Item = Result<T, sqlx::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let s = self.as_mut().get_mut();
        if s.guard.is_none() && !s.done {
            s.guard = Some(Guard::in_context(
                &s.sql,
                s.meta.parameters.clone(),
                s.context.clone(),
            ));
        }
        match s.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(value))) => {
                let count = (s.count)(&value);
                if count < 0 {
                    s.affected = s.affected.saturating_add(-count);
                } else {
                    s.rows = s.rows.saturating_add(count);
                }
                Poll::Ready(Some(Ok(value)))
            }
            Poll::Ready(Some(Err(error))) => {
                if let Some(g) = &mut s.guard {
                    g.finish(false, s.rows, error.to_string());
                }
                s.done = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                if let Some(g) = &mut s.guard {
                    g.finish(
                        true,
                        if s.rows > 0 { s.rows } else { s.affected },
                        String::new(),
                    );
                }
                s.done = true;
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
fn logged<'e, T: Send + 'e>(
    inner: BoxStream<'e, Result<T, sqlx::Error>>,
    sql: String,
    meta: Meta,
    context: super::Context,
    count: fn(&T) -> i64,
) -> BoxStream<'e, Result<T, sqlx::Error>> {
    Box::pin(LoggedStream {
        inner,
        sql,
        meta,
        context,
        guard: None,
        rows: 0,
        affected: 0,
        count,
        done: false,
    })
}

pub struct Query<'q, DB: Database, A> {
    inner: sqlx::query::Query<'q, DB, A>,
    sql: String,
    meta: Meta,
}
pub fn query<DB: Database>(sql: &str) -> Query<'_, DB, DB::Arguments<'_>> {
    Query {
        inner: sqlx::query(sql),
        sql: sql.into(),
        meta: Meta::default(),
    }
}
impl<'q, DB: Database> Query<'q, DB, DB::Arguments<'q>> {
    pub fn bind<T: 'q + Encode<'q, DB> + Type<DB> + std::fmt::Debug>(mut self, value: T) -> Self {
        self.meta.bind(&value);
        self.inner = self.inner.bind(value);
        self
    }
    pub fn bind_sensitive<T: 'q + Encode<'q, DB> + Type<DB>>(mut self, value: T) -> Self {
        self.meta.parameters.push(crate::models::SqlLogParameter {
            ordinal: self.meta.parameters.len() + 1,
            type_name: std::any::type_name::<T>().into(),
            value: "[REDACTED]".into(),
            redacted: true,
        });
        self.inner = self.inner.bind(value);
        self
    }
}
impl<'q, DB, A> Query<'q, DB, A>
where
    DB: Database,
    A: 'q + IntoArguments<'q, DB>,
{
    pub async fn fetch_all<'e, 'c: 'e, E>(self, executor: E) -> Result<Vec<DB::Row>, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_all(executor).await;
        guard.result(&result, |v| v.len() as i64);
        result
    }
    pub async fn fetch_one<'e, 'c: 'e, E>(self, executor: E) -> Result<DB::Row, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_one(executor).await;
        guard.result(&result, |_| 1);
        result
    }
    pub async fn fetch_optional<'e, 'c: 'e, E>(
        self,
        executor: E,
    ) -> Result<Option<DB::Row>, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_optional(executor).await;
        guard.result(&result, |v| i64::from(v.is_some()));
        result
    }
    pub fn fetch<'e, 'c: 'e, E>(self, executor: E) -> BoxStream<'e, Result<DB::Row, sqlx::Error>>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let context = executor.sql_context();
        logged(
            self.inner.fetch(executor),
            self.sql,
            self.meta,
            context,
            |_| 1,
        )
    }
    pub async fn execute<'e, 'c: 'e, E>(self, executor: E) -> Result<DB::QueryResult, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        DB::QueryResult: Affected,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.execute(executor).await;
        guard.result(&result, |v| v.affected().min(i64::MAX as u64) as i64);
        result
    }
    pub fn fetch_many<'e, 'c: 'e, E>(
        self,
        executor: E,
    ) -> BoxStream<'e, Result<sqlx::Either<DB::QueryResult, DB::Row>, sqlx::Error>>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        DB::QueryResult: Affected,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let context = executor.sql_context();
        #[allow(deprecated)]
        let inner = self.inner.fetch_many(executor);
        logged(inner, self.sql, self.meta, context, |v| match v {
            sqlx::Either::Left(r) => -(r.affected().min(i64::MAX as u64) as i64),
            sqlx::Either::Right(_) => 1,
        })
    }
}

pub struct QueryAs<'q, DB: Database, O, A> {
    inner: sqlx::query::QueryAs<'q, DB, O, A>,
    sql: String,
    meta: Meta,
}
pub fn query_as<DB: Database, O>(sql: &str) -> QueryAs<'_, DB, O, DB::Arguments<'_>>
where
    O: for<'r> FromRow<'r, DB::Row>,
{
    QueryAs {
        inner: sqlx::query_as(sql),
        sql: sql.into(),
        meta: Meta::default(),
    }
}
impl<'q, DB: Database, O> QueryAs<'q, DB, O, DB::Arguments<'q>> {
    pub fn bind<T: 'q + Encode<'q, DB> + Type<DB> + std::fmt::Debug>(mut self, value: T) -> Self {
        self.meta.bind(&value);
        self.inner = self.inner.bind(value);
        self
    }
    pub fn bind_sensitive<T: 'q + Encode<'q, DB> + Type<DB>>(mut self, value: T) -> Self {
        self.meta.parameters.push(crate::models::SqlLogParameter {
            ordinal: self.meta.parameters.len() + 1,
            type_name: std::any::type_name::<T>().into(),
            value: "[REDACTED]".into(),
            redacted: true,
        });
        self.inner = self.inner.bind(value);
        self
    }
}
impl<'q, DB, O, A> QueryAs<'q, DB, O, A>
where
    DB: Database,
    A: 'q + IntoArguments<'q, DB>,
    O: Send + Unpin + for<'r> FromRow<'r, DB::Row>,
{
    pub async fn fetch_all<'e, 'c: 'e, E>(self, executor: E) -> Result<Vec<O>, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        O: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_all(executor).await;
        guard.result(&result, |v| v.len() as i64);
        result
    }
    pub async fn fetch_one<'e, 'c: 'e, E>(self, executor: E) -> Result<O, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        O: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_one(executor).await;
        guard.result(&result, |_| 1);
        result
    }
    pub async fn fetch_optional<'e, 'c: 'e, E>(self, executor: E) -> Result<Option<O>, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        O: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_optional(executor).await;
        guard.result(&result, |v| i64::from(v.is_some()));
        result
    }
    pub fn fetch<'e, 'c: 'e, E>(self, executor: E) -> BoxStream<'e, Result<O, sqlx::Error>>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        O: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let context = executor.sql_context();
        logged(
            self.inner.fetch(executor),
            self.sql,
            self.meta,
            context,
            |_| 1,
        )
    }
}

pub struct QueryScalar<'q, DB: Database, O, A> {
    inner: sqlx::query::QueryScalar<'q, DB, O, A>,
    sql: String,
    meta: Meta,
}
pub fn query_scalar<DB: Database, O>(sql: &str) -> QueryScalar<'_, DB, O, DB::Arguments<'_>>
where
    (O,): for<'r> FromRow<'r, DB::Row>,
{
    QueryScalar {
        inner: sqlx::query_scalar(sql),
        sql: sql.into(),
        meta: Meta::default(),
    }
}
impl<'q, DB: Database, O> QueryScalar<'q, DB, O, DB::Arguments<'q>> {
    pub fn bind<T: 'q + Encode<'q, DB> + Type<DB> + std::fmt::Debug>(mut self, value: T) -> Self {
        self.meta.bind(&value);
        self.inner = self.inner.bind(value);
        self
    }
    pub fn bind_sensitive<T: 'q + Encode<'q, DB> + Type<DB>>(mut self, value: T) -> Self {
        self.meta.parameters.push(crate::models::SqlLogParameter {
            ordinal: self.meta.parameters.len() + 1,
            type_name: std::any::type_name::<T>().into(),
            value: "[REDACTED]".into(),
            redacted: true,
        });
        self.inner = self.inner.bind(value);
        self
    }
}
impl<'q, DB, O, A> QueryScalar<'q, DB, O, A>
where
    DB: Database,
    A: 'q + IntoArguments<'q, DB>,
    O: Send + Unpin,
    (O,): for<'r> FromRow<'r, DB::Row>,
{
    pub async fn fetch_all<'e, 'c: 'e, E>(self, executor: E) -> Result<Vec<O>, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        O: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_all(executor).await;
        guard.result(&result, |v| v.len() as i64);
        result
    }
    pub async fn fetch_one<'e, 'c: 'e, E>(self, executor: E) -> Result<O, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        O: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_one(executor).await;
        guard.result(&result, |_| 1);
        result
    }
    pub async fn fetch_optional<'e, 'c: 'e, E>(self, executor: E) -> Result<Option<O>, sqlx::Error>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        O: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let mut guard = Guard::in_context(&self.sql, self.meta.parameters, executor.sql_context());
        let result = self.inner.fetch_optional(executor).await;
        guard.result(&result, |v| i64::from(v.is_some()));
        result
    }
    pub fn fetch<'e, 'c: 'e, E>(self, executor: E) -> BoxStream<'e, Result<O, sqlx::Error>>
    where
        'q: 'e,
        A: 'e,
        DB: 'e,
        O: 'e,
        E: Executor<'c, Database = DB> + ExecutorContext + 'e,
    {
        let context = executor.sql_context();
        logged(
            self.inner.fetch(executor),
            self.sql,
            self.meta,
            context,
            |_| 1,
        )
    }
}

pub struct RawSql<'q> {
    inner: sqlx::RawSql<'q>,
    sql: &'q str,
}
pub fn raw_sql(sql: &str) -> RawSql<'_> {
    RawSql {
        inner: sqlx::raw_sql(sql),
        sql,
    }
}
impl<'q> RawSql<'q> {
    pub fn execute<'e, 'c: 'e, E>(
        self,
        executor: E,
    ) -> futures::future::BoxFuture<'e, Result<<E::Database as Database>::QueryResult, sqlx::Error>>
    where
        'q: 'e,
        E: Executor<'c> + ExecutorContext + 'e,
        <E::Database as Database>::QueryResult: Affected,
    {
        // Erase the generic executor future at this boundary, as SQLx does.
        // This also proves Send here instead of re-inferring HRTB lifetimes in
        // desktop owned-task / async-trait callers of the synchronization path.
        Box::pin(async move {
            let mut guard = Guard::in_context(self.sql, Vec::new(), executor.sql_context());
            let result = sqlx::Executor::execute(executor, self.inner).await;
            guard.result(&result, |v| v.affected().min(i64::MAX as u64) as i64);
            result
        })
    }
}
