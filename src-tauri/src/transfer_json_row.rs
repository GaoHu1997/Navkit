//! Borrowed JSON rows: preserve serde_json's active map ordering without cloning cells.
use serde::{Serialize, Serializer, ser::SerializeMap};

pub(super) fn column_order(names: &[String]) -> Vec<usize> {
    // Cargo feature unification may enable preserve_order through another crate.
    // Prepare the same map ordering once per table rather than assuming BTreeMap.
    let mut map = serde_json::Map::new();
    for (index, name) in names.iter().enumerate() { map.insert(name.clone(), serde_json::Value::from(index)); }
    map.into_iter().map(|(_, index)| index.as_u64().unwrap() as usize).collect()
}

struct JsonRow<'a> {
    names: &'a [String],
    values: &'a [Option<String>],
    order: &'a [usize],
}
impl Serialize for JsonRow<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.order.len()))?;
        for &index in self.order {
            map.serialize_entry(&self.names[index], &self.values[index])?;
        }
        map.end()
    }
}

#[cfg(test)]
tokio::task_local! { static LEGACY_JSON_ROW: bool; }

#[cfg(test)]
pub(super) async fn comparison<F: std::future::Future>(legacy: bool, future: F) -> F::Output {
    LEGACY_JSON_ROW.scope(legacy, future).await
}

pub(super) fn write<W: std::io::Write>(writer: W, names: &[String], values: &[Option<String>], order: &[usize]) -> serde_json::Result<()> {
    #[cfg(test)]
    if LEGACY_JSON_ROW.try_with(|legacy| *legacy).unwrap_or(false) {
        let mut object = serde_json::Map::new();
        for (index, name) in names.iter().enumerate() {
            object.insert(name.clone(), serde_json::Value::from(values.get(index).cloned().flatten()));
        }
        return serde_json::to_writer(writer, &serde_json::Value::Object(object));
    }
    serde_json::to_writer(writer, &JsonRow { names, values, order })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn borrowed_rows_are_byte_identical_for_map_order_unicode_keys_and_all_escapes() {
        let names = vec!["z".into(), "中文".into(), "a\"key".into(), "empty".into(), "nullable".into()];
        let values = vec![Some("\0\u{0001}\t\r\n\\\"é🦀".into()), Some("中文尾".into()), Some("9223372036854775807".into()), Some(String::new()), None];
        let order = column_order(&names);
        let mut legacy = Vec::new();
        comparison(true, async { write(&mut legacy, &names, &values, &order).unwrap(); }).await;
        let mut actual = Vec::new();
        write(&mut actual, &names, &values, &order).unwrap();
        assert_eq!(actual, legacy);
        let row: serde_json::Value = serde_json::from_slice(&actual).unwrap();
        assert_eq!(row["a\"key"], "9223372036854775807");
        assert!(row["nullable"].is_null());
        assert_eq!(row["empty"], "");
    }

    #[tokio::test]
    async fn comparison_policy_is_task_local_not_a_process_switch() {
        let names = vec!["v".into()]; let values = vec![Some("value".into())]; let order = column_order(&names);
        let (old, new) = tokio::join!(comparison(true, async {
            tokio::task::yield_now().await;
            let mut out = Vec::new(); write(&mut out, &names, &values, &order).unwrap(); out
        }), comparison(false, async {
            tokio::task::yield_now().await;
            let mut out = Vec::new(); write(&mut out, &names, &values, &order).unwrap(); out
        }));
        assert_eq!(old, new);
    }
}
