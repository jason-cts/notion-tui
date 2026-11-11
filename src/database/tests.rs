use super::*;
use serde_json::json;

fn data(kind: &str, mode: Value, values: Vec<Value>) -> DatabaseData {
    let actual = match kind {
        "person" => "people",
        "text" => "rich_text",
        k => k,
    };
    DatabaseData {
        view:serde_json::from_value(json!({"id":"v","type":"board","data_source_id":"source","configuration":{"type":"board","group_by":{"type":kind,"property_id":"p","group_by":mode,"sort":{"type":"ascending"}}}})).unwrap(),
        schema:json!({"properties":{"P":{"id":"p","type":actual}}}),
        rows:values.into_iter().enumerate().map(|(i,value)|DatabaseRow{id:i.to_string(),title:i.to_string(),page:json!({"id":i.to_string(),"properties":{"P":{"id":"p","type":actual,actual:value}}})}).collect(),labels:HashMap::new(),notices:vec![],
    }
}
fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 11).unwrap()
}
fn labels(data: &DatabaseData) -> Vec<String> {
    data.groups(today())
        .iter()
        .map(|g| g.label.clone())
        .collect()
}

#[test]
fn scalar_and_formula_groups_keep_values_and_empty_entries() {
    for kind in ["title", "text", "url", "email", "phone_number"] {
        let values = if matches!(kind, "title" | "text") {
            vec![json!([{"text":{"content":"Alpha"}}]), json!([])]
        } else {
            vec![json!("Alpha"), Value::Null]
        };
        assert_eq!(
            labels(&data(kind, json!("exact"), values)),
            ["Alpha", "No value"]
        );
    }
    assert_eq!(
        labels(&data(
            "text",
            json!("alphabet_prefix"),
            vec![json!([{"plain_text":"🦀 hi"}])]
        )),
        ["🦀"]
    );
    assert_eq!(
        labels(&data(
            "checkbox",
            Value::Null,
            vec![json!(true), json!(false)]
        )),
        ["Unchecked", "Checked"]
    );
    assert_eq!(
        labels(&data(
            "number",
            Value::Null,
            vec![json!(10), json!(2), Value::Null]
        )),
        ["2", "10", "No value"]
    );
    for (kind, value, label) in [
        ("text", json!({"type":"string","string":"Alpha"}), "Alpha"),
        ("number", json!({"type":"number","number":2}), "2"),
        (
            "checkbox",
            json!({"type":"boolean","boolean":true}),
            "Checked",
        ),
        (
            "date",
            json!({"type":"date","date":{"start":"2026-11-11"}}),
            "2026-11-11",
        ),
    ] {
        let expected = if kind == "checkbox" {
            vec!["Unchecked", label, "No value"]
        } else {
            vec![label, "No value"]
        };
        assert_eq!(
            labels(&data(
                "formula",
                json!({"type":kind,"group_by":"day"}),
                vec![value, Value::Null]
            )),
            expected
        );
    }
}

#[test]
fn arrays_group_by_identity_and_deduplicate_membership() {
    for kind in ["multi_select", "person", "relation"] {
        let d = data(
            kind,
            Value::Null,
            vec![
                json!([{"id":"a","name":"A"},{"id":"b","name":"B"},{"id":"a","name":"A"}]),
                json!([]),
            ],
        );
        let groups = d.groups(today());
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].cards.len(), 1);
        assert_eq!(groups[1].cards[0].row, 0);
        assert_eq!(groups[2].label, "No value");
    }
    for kind in ["select", "created_by", "last_edited_by"] {
        assert_eq!(
            labels(&data(
                kind,
                Value::Null,
                vec![json!({"id":"a","name":"Alice"})]
            )),
            ["Alice"]
        );
    }
}

#[test]
fn status_group_mapping_and_hidden_empty_groups_use_schema() {
    let mut d = data(
        "status",
        json!("group"),
        vec![json!({"id":"doing","name":"Working"})],
    );
    d.schema["properties"]["P"]["status"] = json!({"groups":[{"id":"todo","name":"To do","option_ids":[]},{"id":"progress","name":"In progress","option_ids":["doing"]}]});
    let g = d
        .view
        .configuration
        .as_mut()
        .unwrap()
        .group_by
        .as_mut()
        .unwrap();
    g.sort.as_mut().unwrap().sort_type = "manual".into();
    assert_eq!(labels(&d), ["To do", "In progress"]);
    d.view
        .configuration
        .as_mut()
        .unwrap()
        .group_by
        .as_mut()
        .unwrap()
        .hide_empty_groups = Some(true);
    assert_eq!(labels(&d), ["In progress"]);
}

#[test]
fn number_buckets_use_exclusive_upper_bounds_and_numeric_order() {
    let mut d = data(
        "number",
        Value::Null,
        vec![json!(-1), json!(0), json!(9.99), json!(10), json!(20)],
    );
    let g = d
        .view
        .configuration
        .as_mut()
        .unwrap()
        .group_by
        .as_mut()
        .unwrap();
    g.range_start = Some(0.);
    g.range_end = Some(20.);
    g.range_size = Some(10.);
    assert_eq!(labels(&d), ["< 0", "0 – 10", "10 – 20", "≥ 20"]);
    assert_eq!(d.groups(today())[1].cards.len(), 2);
}

#[test]
fn date_modes_week_starts_and_relative_boundaries_are_fixed() {
    for kind in ["date", "created_time", "last_edited_time"] {
        let value = if kind == "date" {
            json!({"start":"2026-11-08","end":"2026-12-31"})
        } else {
            json!("2026-11-08T12:00:00Z")
        };
        let mut d = data(kind, json!("week"), vec![value]);
        assert_eq!(labels(&d), ["2026-11-02"]);
        d.view
            .configuration
            .as_mut()
            .unwrap()
            .group_by
            .as_mut()
            .unwrap()
            .start_day_of_week = Some(0);
        assert_eq!(labels(&d), ["2026-11-08"]);
    }
    for (mode, expected) in [
        ("day", "2026-11-11"),
        ("month", "2026-11"),
        ("year", "2026"),
    ] {
        assert_eq!(
            labels(&data(
                "date",
                json!(mode),
                vec![json!({"start":"2026-11-11"})]
            )),
            [expected]
        );
    }
    assert_eq!(
        labels(&data(
            "date",
            json!("relative"),
            vec![
                json!({"start":"2026-11-10"}),
                json!({"start":"2026-11-11"}),
                json!({"start":"2026-11-12"})
            ]
        )),
        ["Yesterday", "Today", "Tomorrow"]
    );
}

#[test]
fn timestamps_use_local_dates_instead_of_the_written_calendar_date() {
    let instant = "2026-11-10T23:30:00-08:00";
    let expected = DateTime::parse_from_rfc3339(instant)
        .unwrap()
        .with_timezone(&Local)
        .date_naive()
        .to_string();
    let d = data("date", json!("day"), vec![json!({"start":instant})]);
    assert_eq!(labels(&d), [expected]);
}

#[test]
fn saved_columns_styles_links_and_board_navigation_are_preserved() {
    let view = crate::service::database::demo_metadata("demo")
        .views
        .remove(2);
    let mut d = crate::service::database::demo_data(view);
    d.view
        .configuration
        .as_mut()
        .unwrap()
        .properties
        .as_mut()
        .unwrap()[1]
        .visible = Some(false);
    let columns = d.columns();
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0].width, 25);
    assert!(
        d.cell(&d.rows[0], &columns[0])[0]
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
    assert_eq!(
        d.cell(&d.rows[0], &columns[1])[0].url.as_deref(),
        Some("https://example.com/")
    );
    let view = crate::service::database::demo_metadata("demo")
        .views
        .remove(1);
    let d = crate::service::database::demo_data(view);
    let mut state = DatabaseState {
        data: Some(d),
        ..Default::default()
    };
    state.move_horizontal(true);
    assert_eq!(state.selected, 1);
    assert_eq!(state.column, 1);
    state.move_horizontal(true);
    assert_eq!(state.selected, 2);
    state.move_horizontal(false);
    assert_eq!(state.selected, 1);
}

#[test]
fn subgroups_preserve_numeric_order_empty_options_and_duplicate_card_focus() {
    let mut d = data(
        "multi_select",
        Value::Null,
        vec![
            json!([{"id":"a","name":"A"},{"id":"b","name":"B"}]),
            json!([{"id":"a","name":"A"}]),
        ],
    );
    d.schema["properties"]["Sub"] = json!({"id":"sub","type":"multi_select","multi_select":{"options":[{"id":"x","name":"X"},{"id":"y","name":"Y"},{"id":"z","name":"Empty"}]}});
    d.rows[0].page["properties"]["Sub"] = json!({"id":"sub","type":"multi_select","multi_select":[{"id":"x","name":"X"},{"id":"y","name":"Y"}]});
    d.rows[1].page["properties"]["Sub"] =
        json!({"id":"sub","type":"multi_select","multi_select":[{"id":"x","name":"X"}]});
    d.view.configuration.as_mut().unwrap().sub_group_by = Some(
        serde_json::from_value(
            json!({"type":"multi_select","property_id":"sub","sort":{"type":"manual"}}),
        )
        .unwrap(),
    );
    let groups = d.groups(today());
    assert_eq!(
        groups[0]
            .subgroups
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>(),
        ["X", "Y", "Empty"]
    );
    assert_eq!(
        groups[0].cards.iter().map(|c| c.row).collect::<Vec<_>>(),
        [0, 1, 0]
    );
    let mut state = DatabaseState {
        data: Some(d),
        ..Default::default()
    };
    state.move_vertical(true);
    assert_eq!(state.selected, 1);
    assert_eq!(state.card, 1);
    state.move_vertical(true);
    assert_eq!(state.selected, 0);
    assert_eq!(state.card, 2);
    state.move_horizontal(true);
    assert_eq!(state.column, 1);
    assert_eq!(state.selected, 0);
    state.move_edge(true);
    assert_eq!(state.card, 1);
    let d = state.data.as_mut().unwrap();
    d.view.configuration.as_mut().unwrap().sub_group_by = Some(
        serde_json::from_value(
            json!({"type":"number","property_id":"sub","sort":{"type":"ascending"}}),
        )
        .unwrap(),
    );
    d.rows[0].page["properties"]["Sub"] = json!({"id":"sub","type":"number","number":10});
    d.rows[1].page["properties"]["Sub"] = json!({"id":"sub","type":"number","number":2});
    assert_eq!(
        d.groups(today())[0]
            .cards
            .iter()
            .map(|c| c.row)
            .collect::<Vec<_>>(),
        [1, 0]
    );
}

#[test]
fn empty_board_columns_never_open_a_different_columns_page() {
    let mut d = data("checkbox", Value::Null, vec![json!(true)]);
    d.view
        .configuration
        .as_mut()
        .unwrap()
        .group_by
        .as_mut()
        .unwrap()
        .sort
        .as_mut()
        .unwrap()
        .sort_type = "manual".into();
    let mut state = DatabaseState {
        column: 1,
        data: Some(d),
        ..Default::default()
    };
    state.move_horizontal(false);
    assert!(state.selected_row().is_none());
    state.move_vertical(true);
    assert!(state.selected_row().is_none());
    state.move_horizontal(true);
    assert_eq!(state.selected_row().unwrap().id, "0");
}

#[test]
fn encoded_property_ids_match_columns_cells_status_groups_and_subgroups() {
    for (raw, encoded) in [
        ("tG`_", "tG%60_"),
        ("g?Nq", "g%3fNq"),
        ("a+b;", "a%2Bb%3B"),
        ("p%id", "p%25id"),
    ] {
        assert!(same_property_id(raw, encoded));
        assert!(same_property_id(encoded, raw));
        assert_eq!(property_id_key(raw), property_id_key(encoded));
    }
    assert!(!same_property_id("a+b", "a b"));
    assert!(!same_property_id("g?Nq", "g%253FNq"));
    let mut d = data(
        "status",
        json!("option"),
        vec![
            json!({"id":"done","name":"Done"}),
            json!({"id":"doing","name":"In progress"}),
            json!({"id":"todo","name":"Not started"}),
        ],
    );
    d.schema = json!({"properties": {
        "Status":{"id":"tG%60_","type":"status","status":{"options":[
            {"id":"todo","name":"Not started"},{"id":"doing","name":"In progress"},{"id":"done","name":"Done"}
        ]}}, "Checkbox":{"id":"g%3FNq","type":"checkbox"}
    }});
    for row in &mut d.rows {
        let value = row.page["properties"]["P"]["status"].clone();
        row.page["properties"] = json!({"Status":{"id":"tG%60_","type":"status","status":value},
            "Checkbox":{"id":"g%3FNq","type":"checkbox","checkbox":true}});
    }
    let config = d.view.configuration.as_mut().unwrap();
    config.group_by.as_mut().unwrap().property_id = "tG`_".into();
    config.properties = Some(
        serde_json::from_value(json!([
            {"property_id":"g?Nq","visible":true},{"property_id":"tG`_","visible":false}
        ]))
        .unwrap(),
    );
    let columns = d.columns();
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0].name, "Checkbox");
    assert_eq!(d.cell(&d.rows[0], &columns[0])[0].text, "☑");
    assert_eq!(labels(&d), ["Not started", "In progress", "Done"]);
    let groups = d.groups(today());
    assert_eq!(
        groups.iter().map(|g| g.cards[0].row).collect::<Vec<_>>(),
        [2, 1, 0]
    );
    d.view
        .configuration
        .as_mut()
        .unwrap()
        .group_by
        .as_mut()
        .unwrap()
        .sort
        .as_mut()
        .unwrap()
        .sort_type = "descending".into();
    assert_eq!(labels(&d), ["Done", "In progress", "Not started"]);
    d.view.configuration.as_mut().unwrap().sub_group_by = Some(serde_json::from_value(json!({
        "type":"checkbox","property_id":"g?Nq","hide_empty_groups":true,"sort":{"type":"ascending"}
    })).unwrap());
    for group in d.groups(today()) {
        assert_eq!(group.subgroups, [("true".into(), "Checked".into())]);
    }
}
