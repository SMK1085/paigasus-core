// SPDX-License-Identifier: Apache-2.0

use jiff::{Timestamp, tz::TimeZone};
use serde_json::Value;
use uuid::Uuid;

use crate::event::{ID_MAX_CHARS, TYPE_MAX_CHARS, URI_MAX_CHARS, check_absolute_uri, check_extension_name, check_extension_value, check_text};
use crate::{CloudEvent, ExtensionValue, SchemaError};

impl CloudEvent {
    /// Make an event with the four required attributes and a time.
    ///
    /// # Errors
    /// Returns an error when `source` or `ty` is empty or too long.
    pub fn new(id: Uuid, source: &str, ty: &str, time: Timestamp) -> Result<CloudEvent, SchemaError> {
        CloudEvent::new_text_id(&id.hyphenated().to_string(), source, ty, time)
    }

    /// Make an event whose `id` is text, not a UUID. The `id` gets the same checks as in
    /// [`CloudEvent::new`].
    ///
    /// # Errors
    /// Returns an error when `id`, `source` or `ty` is empty, too long, or has a character that
    /// CloudEvents does not allow.
    pub fn new_text_id(id: &str, source: &str, ty: &str, time: Timestamp) -> Result<CloudEvent, SchemaError> {
        check_text("id", id, ID_MAX_CHARS)?;
        check_text("source", source, URI_MAX_CHARS)?;
        check_text("type", ty, TYPE_MAX_CHARS)?;
        Ok(CloudEvent {
            id: id.to_string(),
            source: source.to_string(),
            ty: ty.to_string(),
            time: Some(time),
            subject: None,
            datacontenttype: None,
            dataschema: None,
            data: None,
            extensions: Vec::new(),
        })
    }

    /// # Errors
    /// Returns an error when the value is empty or too long.
    pub fn with_subject(mut self, subject: &str) -> Result<Self, SchemaError> {
        check_text("subject", subject, URI_MAX_CHARS)?;
        self.subject = Some(subject.to_string());
        Ok(self)
    }

    /// # Errors
    /// Returns an error when the value is empty or too long.
    pub fn with_datacontenttype(mut self, value: &str) -> Result<Self, SchemaError> {
        check_text("datacontenttype", value, URI_MAX_CHARS)?;
        self.datacontenttype = Some(value.to_string());
        Ok(self)
    }

    /// # Errors
    /// Returns an error when the value is empty, too long, or not an absolute URI.
    pub fn with_dataschema(mut self, value: &str) -> Result<Self, SchemaError> {
        check_text("dataschema", value, URI_MAX_CHARS)?;
        check_absolute_uri("dataschema", value)?;
        self.dataschema = Some(value.to_string());
        Ok(self)
    }

    /// Set `data`. The build side does not check it: `parse` refuses a `data` with U+0000, so a
    /// host-built event with such data cannot be read back.
    #[must_use]
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Set an extension. A new name goes last. An existing name keeps its place.
    ///
    /// # Errors
    /// Returns an error when the name or the value breaks a limit.
    pub fn set_extension(&mut self, name: &str, value: ExtensionValue) -> Result<(), SchemaError> {
        check_extension_name(name)?;
        check_extension_value(name, &value)?;
        match self.extensions.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => *v = value,
            None => self.extensions.push((name.to_string(), value)),
        }
        Ok(())
    }

    pub fn remove_extension(&mut self, name: &str) {
        self.extensions.retain(|(n, _)| n != name);
    }

    /// # Errors
    /// Returns an error when the value is empty or too long.
    pub fn set_source(&mut self, source: &str) -> Result<(), SchemaError> {
        check_text("source", source, URI_MAX_CHARS)?;
        self.source = source.to_string();
        Ok(())
    }

    /// The event as a JSON value. With the default `serde_json` map, the key order is
    /// alphabetical. Use [`CloudEvent::to_bytes`] when the order matters.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        for (key, value) in self.entries() {
            map.insert(key, value);
        }
        Value::Object(map)
    }

    /// The event as JSON bytes in a fixed attribute order: `specversion`, `id`, `source`,
    /// `type`, `subject`, `time`, `datacontenttype`, `dataschema`, the extensions in insertion
    /// order, `data`. This is the order of the IAM encoder.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::from("{");
        for (i, (key, value)) in self.entries().into_iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&Value::String(key).to_string());
            out.push(':');
            out.push_str(&value.to_string());
        }
        out.push('}');
        out.into_bytes()
    }

    fn entries(&self) -> Vec<(String, Value)> {
        let text = |s: &str| Value::String(s.to_string());
        let mut e: Vec<(String, Value)> = vec![
            ("specversion".into(), text("1.0")),
            ("id".into(), text(&self.id)),
            ("source".into(), text(&self.source)),
            ("type".into(), text(&self.ty)),
        ];
        if let Some(s) = &self.subject {
            e.push(("subject".into(), text(s)));
        }
        if let Some(t) = self.time {
            e.push(("time".into(), text(&format_time(t))));
        }
        if let Some(s) = &self.datacontenttype {
            e.push(("datacontenttype".into(), text(s)));
        }
        if let Some(s) = &self.dataschema {
            e.push(("dataschema".into(), text(s)));
        }
        for (name, value) in &self.extensions {
            let v = match value {
                ExtensionValue::String(s) => text(s),
                ExtensionValue::Integer(i) => Value::from(*i),
                ExtensionValue::Boolean(b) => Value::Bool(*b),
            };
            e.push((name.clone(), v));
        }
        if let Some(d) = &self.data {
            e.push(("data".into(), d.clone()));
        }
        e
    }
}

/// RFC 3339 in UTC with exactly six fraction digits. Finer digits are cut, not rounded.
fn format_time(t: Timestamp) -> String {
    let z = t.to_zoned_datetime(TimeZone::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        z.year(),
        z.month(),
        z.day(),
        z.hour(),
        z.minute(),
        z.second(),
        z.subsec_nanosecond() / 1000
    )
}

#[cfg(test)]
mod tests {
    use crate::{CloudEvent, ExtensionValue, SchemaError};
    use jiff::Timestamp;
    use uuid::Uuid;

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    #[test]
    fn new_text_id_checks_the_id() {
        let ok = "t".repeat(63);
        assert_eq!(CloudEvent::new_text_id(&ok, "urn:s", "t", ts("2026-03-29T01:00:00Z")).unwrap().id(), ok);
        assert!(matches!(CloudEvent::new_text_id("", "urn:s", "t", ts("2026-03-29T01:00:00Z")), Err(SchemaError::Empty("id"))));
        assert!(matches!(
            CloudEvent::new_text_id(&"a".repeat(257), "urn:s", "t", ts("2026-03-29T01:00:00Z")),
            Err(SchemaError::TooLong { .. })
        ));
        assert!(matches!(
            CloudEvent::new_text_id("a\nb", "urn:s", "t", ts("2026-03-29T01:00:00Z")),
            Err(SchemaError::InvalidCharacter(_))
        ));
    }

    fn event_at(time: &str) -> CloudEvent {
        CloudEvent::new(Uuid::from_u128(0x1234), "urn:s", "t", ts(time)).unwrap()
    }

    fn time_text(e: &CloudEvent) -> String {
        e.to_json()["time"].as_str().unwrap().to_string()
    }

    #[test]
    fn build_prints_six_fraction_digits_and_truncates() {
        assert_eq!(time_text(&event_at("2026-08-07T12:30:45.123456789Z")), "2026-08-07T12:30:45.123456Z");
        assert_eq!(time_text(&event_at("2026-08-07T12:30:45.1Z")), "2026-08-07T12:30:45.100000Z");
        assert_eq!(time_text(&event_at("2026-08-07T12:30:45Z")), "2026-08-07T12:30:45.000000Z");
    }

    #[test]
    fn build_writes_a_hyphenated_uuid_id() {
        assert_eq!(event_at("2026-08-07T12:30:45Z").id(), "00000000-0000-0000-0000-000000001234");
    }

    #[test]
    fn set_extension_keeps_insertion_order() {
        let mut e = event_at("2026-08-07T12:30:45Z");
        e.set_extension("schemaversion", ExtensionValue::Integer(1)).unwrap();
        e.set_extension("actorprn", ExtensionValue::String("a".into())).unwrap();
        e.set_extension("correlationid", ExtensionValue::String("c".into())).unwrap();
        // A replace keeps the position.
        e.set_extension("schemaversion", ExtensionValue::Integer(2)).unwrap();
        let text = String::from_utf8(e.to_bytes()).unwrap();
        let s = text.find("\"schemaversion\":2").unwrap();
        let a = text.find("\"actorprn\"").unwrap();
        let c = text.find("\"correlationid\"").unwrap();
        assert!(s < a && a < c, "{text}");
    }

    #[test]
    fn with_dataschema_needs_an_absolute_uri() {
        let e = event_at("2026-08-07T12:30:45Z");
        assert!(e.clone().with_dataschema("https://example.com/order.json").is_ok());
        assert_eq!(e.clone().with_dataschema("schemas/order.json").err(), Some(SchemaError::NotAbsoluteUri("dataschema")));
        assert_eq!(e.clone().with_dataschema("https://example.com/[schema]").err(), Some(SchemaError::NotAbsoluteUri("dataschema")));
        assert!(e.with_dataschema("https://[::1]:8443/order.json").is_ok());
    }

    #[test]
    fn set_extension_refuses_a_control_character() {
        let mut e = event_at("2026-08-07T12:30:45Z");
        assert_eq!(
            e.set_extension("correlationid", ExtensionValue::String("a\u{0}b".into())),
            Err(SchemaError::InvalidCharacter("correlationid".into()))
        );
    }

    #[test]
    fn set_source_checks_the_length_limit() {
        let mut e = event_at("2026-08-07T12:30:45Z");
        assert!(matches!(e.set_source(&"x".repeat(1025)), Err(SchemaError::TooLong { .. })));
        e.set_source(&"x".repeat(1024)).unwrap();
    }
}
