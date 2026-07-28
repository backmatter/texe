use std::collections::BTreeSet;

#[test]
fn machine_protocol_schemas_are_closed_and_validate_v1_golden_artifacts() {
    let cases = [
        (
            include_str!("../schemas/texe.build-report.schema.json"),
            include_str!("golden/v1/build-report.json"),
        ),
        (
            include_str!("../schemas/texe.clean.schema.json"),
            include_str!("golden/v1/clean-report.json"),
        ),
        (
            include_str!("../schemas/texe.clean.schema.json"),
            include_str!("golden/v1/clean-dry-run.json"),
        ),
        (
            include_str!("../schemas/texe.storage-report.schema.json"),
            include_str!("golden/v1/storage-report.json"),
        ),
        (
            include_str!("../schemas/texe.init-report.schema.json"),
            include_str!("golden/v1/init-report.json"),
        ),
        (
            include_str!("../schemas/texe.bare-report.schema.json"),
            include_str!("golden/v1/bare-report.json"),
        ),
        (
            include_str!("../schemas/texe.doctor-report.schema.json"),
            include_str!("golden/v1/doctor-report.json"),
        ),
        (
            include_str!("../schemas/texe.editor-report.schema.json"),
            include_str!("golden/v1/editor-report.json"),
        ),
        (
            include_str!("../schemas/texe.viewer-status.schema.json"),
            include_str!("golden/v1/viewer-status.json"),
        ),
    ];

    let mut checked_schemas = BTreeSet::new();
    for (schema_text, artifact_text) in cases {
        let schema: serde_json::Value =
            serde_json::from_str(schema_text).expect("schema is valid JSON");
        if checked_schemas.insert(schema["$id"].as_str().expect("schema ID").to_string()) {
            jsonschema::draft202012::meta::validate(&schema)
                .unwrap_or_else(|error| panic!("invalid JSON Schema: {error}"));
            assert_closed_objects(&schema, "#");
        }
        let artifact: serde_json::Value =
            serde_json::from_str(artifact_text).expect("golden artifact is valid JSON");
        jsonschema::draft202012::validate(&schema, &artifact)
            .unwrap_or_else(|error| panic!("golden artifact violates its schema: {error}"));
    }
}

#[test]
fn every_public_schema_has_a_canonical_raw_github_id() {
    for (filename, text) in [
        (
            "texe.bare-report.schema.json",
            include_str!("../schemas/texe.bare-report.schema.json"),
        ),
        (
            "texe.build-report.schema.json",
            include_str!("../schemas/texe.build-report.schema.json"),
        ),
        (
            "texe.clean.schema.json",
            include_str!("../schemas/texe.clean.schema.json"),
        ),
        (
            "texe.doctor-report.schema.json",
            include_str!("../schemas/texe.doctor-report.schema.json"),
        ),
        (
            "texe.editor-report.schema.json",
            include_str!("../schemas/texe.editor-report.schema.json"),
        ),
        (
            "texe.error.schema.json",
            include_str!("../schemas/texe.error.schema.json"),
        ),
        (
            "texe.init-report.schema.json",
            include_str!("../schemas/texe.init-report.schema.json"),
        ),
        (
            "texe.lock.schema.json",
            include_str!("../schemas/texe.lock.schema.json"),
        ),
        (
            "texe.project.schema.json",
            include_str!("../schemas/texe.project.schema.json"),
        ),
        (
            "texe.storage-report.schema.json",
            include_str!("../schemas/texe.storage-report.schema.json"),
        ),
        (
            "texe.viewer-status.schema.json",
            include_str!("../schemas/texe.viewer-status.schema.json"),
        ),
        (
            "texe.watch-event.schema.json",
            include_str!("../schemas/texe.watch-event.schema.json"),
        ),
    ] {
        let schema: serde_json::Value = serde_json::from_str(text).expect("schema is valid JSON");
        assert_eq!(
            schema["$id"],
            format!("https://raw.githubusercontent.com/backmatter/texe/main/schemas/{filename}")
        );
        jsonschema::draft202012::meta::validate(&schema)
            .unwrap_or_else(|error| panic!("{filename} is not valid JSON Schema: {error}"));
        assert_closed_objects(&schema, "#");
    }
}

fn assert_closed_objects(value: &serde_json::Value, path: &str) {
    match value {
        serde_json::Value::Object(object) => {
            if object.get("type").and_then(serde_json::Value::as_str) == Some("object") {
                assert_eq!(
                    object.get("additionalProperties"),
                    Some(&serde_json::Value::Bool(false)),
                    "object schema at {path} is open"
                );
            }
            for (key, nested) in object {
                assert_closed_objects(nested, &format!("{path}/{key}"));
            }
        }
        serde_json::Value::Array(values) => {
            for (index, nested) in values.iter().enumerate() {
                assert_closed_objects(nested, &format!("{path}/{index}"));
            }
        }
        _ => {}
    }
}
