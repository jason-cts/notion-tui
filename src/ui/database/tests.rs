use super::*;
use crate::{
    app::{Location, LocationKind},
    service::{
        DemoService,
        database::{demo_data, demo_metadata},
    },
};
use ratatui::{Terminal, backend::TestBackend};
use std::sync::Arc;

#[test]
fn table_overflow_indicators_follow_focus_clipping_and_resize() {
    let meta = demo_metadata("db");
    let mut value = serde_json::to_value(&meta.views[2]).unwrap();
    for p in value["configuration"]["properties"].as_array_mut().unwrap() {
        p["width"] = serde_json::json!(80);
    }
    let data = demo_data(serde_json::from_value(value).unwrap());
    for (width, cell, left, right) in [
        (60, 0, false, false),
        (17, 0, false, true),
        (17, 2, true, true),
        (17, 3, true, false),
        (10, 0, false, true),
    ] {
        let layout = table_layout(&data, cell, Rect::new(0, 0, width, 8));
        assert_eq!(
            (layout.hidden_left, layout.hidden_right),
            (left, right),
            "width {width} cell {cell}"
        );
        let db = DatabaseState {
            table_cell: cell,
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(width, 8)).unwrap();
        terminal
            .draw(|f| Ui::default().database_table(f, &data, &db, f.area(), "table"))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer[(layout.left.unwrap(), 0)].symbol(),
            if left { "<" } else { " " }
        );
        assert_eq!(
            buffer[(layout.right.unwrap(), 0)].symbol(),
            if right { ">" } else { " " }
        );
    }
    for width in 1..6 {
        table_layout(&data, 3, Rect::new(0, 0, width, 1));
    }
}

#[test]
fn table_highlights_only_one_wrapped_cell_and_pins_page_icons() {
    let meta = demo_metadata("db");
    let mut data = demo_data(meta.views[2].clone());
    data.rows[0].page["icon"] = serde_json::json!({"type":"emoji","emoji":"🦀"});
    data.view.configuration.as_mut().unwrap().wrap_cells = Some(true);
    data.rows[0].page["properties"]["Name"]["title"][0]["plain_text"] =
        serde_json::json!("First\nSecond\nThird");
    let db = DatabaseState {
        table_cell: 2,
        ..Default::default()
    };
    let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
    terminal
        .draw(|f| Ui::default().database_table(f, &data, &db, f.area(), "table"))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let layout = table_layout(&data, 2, Rect::new(0, 0, 90, 12));
    let selected = layout.columns.iter().find(|(i, _)| *i == 1).unwrap().1;
    assert_eq!(buffer[(0, 1)].symbol(), "🦀");
    assert_eq!(buffer[(0, 4)].symbol(), "📄");
    let highlighted = buffer
        .content
        .iter()
        .filter(|c| c.bg == Color::Rgb(43, 53, 66))
        .count();
    assert_eq!(highlighted, usize::from(selected.width) * 3);
    for y in 1..4 {
        assert_eq!(buffer[(selected.x, y)].bg, Color::Rgb(43, 53, 66));
        assert_eq!(buffer[(0, y)].bg, PANEL);
    }
    let db = DatabaseState {
        table_cell: 0,
        ..Default::default()
    };
    terminal
        .draw(|f| Ui::default().database_table(f, &data, &db, f.area(), "table"))
        .unwrap();
    assert_eq!(
        terminal.backend().buffer()[(0, 1)].bg,
        Color::Rgb(43, 53, 66)
    );
}

#[test]
fn repeated_grouped_pages_highlight_one_occurrence_and_empty_schemas_keep_icons() {
    let meta = demo_metadata("db");
    let mut data = demo_data(meta.views[2].clone());
    let mut view = serde_json::to_value(&data.view).unwrap();
    view["configuration"]["group_by"] =
        serde_json::json!({"type":"multi_select","property_id":"tags"});
    data.view = serde_json::from_value(view).unwrap();
    data.schema["properties"]["Tags"] = serde_json::json!({"id":"tags","type":"multi_select"});
    data.rows.truncate(1);
    data.rows[0].page["properties"]["Tags"] = serde_json::json!({"id":"tags","type":"multi_select","multi_select":[{"id":"a","name":"Alpha"},{"id":"b","name":"Beta"}]});
    assert_eq!(data.table_rows().len(), 2);
    let db = DatabaseState {
        table_row: 1,
        table_cell: 2,
        ..Default::default()
    };
    let mut terminal = Terminal::new(TestBackend::new(90, 12)).unwrap();
    terminal
        .draw(|f| Ui::default().database_table(f, &data, &db, f.area(), "table"))
        .unwrap();
    let width = table_layout(&data, 2, Rect::new(0, 0, 90, 12))
        .columns
        .iter()
        .find(|(i, _)| *i == 1)
        .unwrap()
        .1
        .width;
    assert_eq!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .filter(|c| c.bg == Color::Rgb(43, 53, 66))
            .count(),
        usize::from(width)
    );
    data.schema = serde_json::json!({"properties":{}});
    let db = DatabaseState::default();
    terminal
        .draw(|f| Ui::default().database_table(f, &data, &db, f.area(), "table"))
        .unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .any(|c| c.symbol() == "📄")
    );
}

#[test]
fn cell_editors_render_at_small_sizes_and_keep_the_active_field_visible() {
    for kind in ["date", "status", "multi_select", "checkbox", "rich_text"] {
        let meta = demo_metadata("db");
        let mut data = demo_data(meta.views[2].clone());
        data.schema["properties"]["P"] =
            serde_json::json!({"id":"p","type":kind,kind:{"options":[{"id":"a","name":"Alpha"}]}});
        data.rows[0].page["properties"]["P"] = serde_json::json!({"id":"p","type":kind,kind: match kind { "rich_text" | "multi_select" => serde_json::json!([]), "checkbox"=>serde_json::json!(false), _=>serde_json::Value::Null }});
        let target = crate::editor::CellTarget::new(
            &data,
            &data.rows[0],
            Column {
                id: "p".into(),
                name: "P".into(),
                width: 20,
                wrap: true,
            },
        )
        .unwrap();
        let mut editor = Editor::new(Target::DatabaseCell(Box::new(target)));
        if kind == "date" {
            editor.cell.as_mut().unwrap().active = 2;
        }
        for (w, h) in [(100, 30), (25, 10), (8, 6), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| render_cell_editor(f, &editor, false, true))
                .unwrap();
            if kind == "date" && w == 100 {
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|c| c.symbol())
                        .collect::<String>()
                        .contains("Timezone")
                );
            }
            editor.error = Some("Save failed; draft retained".into());
            terminal
                .draw(|f| render_cell_editor(f, &editor, false, true))
                .unwrap();
        }
    }
}

#[test]
fn all_database_layouts_render_unicode_styles_links_and_narrow_sizes() {
    for index in 0..3 {
        for (width, height) in [(120, 28), (55, 15), (24, 10)] {
            let (sender, _) = tokio::sync::mpsc::unbounded_channel();
            let mut app = App::new(Arc::new(DemoService), sender, true);
            app.focus = Focus::Content;
            app.path.push(Location {
                id: "db".into(),
                title: "Tasks".into(),
                kind: LocationKind::Database,
                page_id: "parent".into(),
                list_depth: 0,
            });
            let meta = demo_metadata("db");
            let data = demo_data(meta.views[index].clone());
            app.databases.insert(
                "db".into(),
                DatabaseState {
                    meta: Some(meta),
                    data: Some(data),
                    view_index: index,
                    ..Default::default()
                },
            );
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut ui = Ui::default();
            if index == 1 && width == 120 {
                let db = app.databases.get_mut("db").unwrap();
                db.column = 2;
                db.selected = 2;
            }
            terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
            let text = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                text.contains("Alpha"),
                "layout {index}, {width} columns: {text}"
            );
            if width == 120 {
                if index == 1 {
                    for column in ["To do (1)", "In progress (1)", "Done (1)"] {
                        assert!(
                            text.contains(column),
                            "all workflow columns should fit: {text}"
                        );
                    }
                }
                assert!(text.contains("child_database"));
                assert!(
                    !ui.links.is_empty(),
                    "layout {index} must record external links"
                );
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .any(|c| c.modifier.contains(Modifier::BOLD))
                );
            }
            app.databases.get_mut("db").unwrap().move_horizontal(true);
            terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
        }
    }
}

#[test]
fn empty_and_failed_views_show_database_specific_messages() {
    for failed in [false, true] {
        let (sender, _) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(Arc::new(DemoService), sender, true);
        app.path.push(Location {
            id: "db".into(),
            title: "Tasks".into(),
            kind: LocationKind::Database,
            page_id: "parent".into(),
            list_depth: 0,
        });
        let meta = demo_metadata("db");
        let mut data = demo_data(meta.views[0].clone());
        data.rows.clear();
        app.databases.insert(
            "db".into(),
            DatabaseState {
                meta: Some(meta),
                data: Some(data),
                error: failed.then(|| "Access denied".into()),
                ..Default::default()
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|frame| Ui::default().draw(frame, &app))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains(if failed {
            "Access denied"
        } else {
            "No rows match"
        }));
        assert!(!text.contains("a append a block"));
    }
}

#[test]
fn tables_with_negative_one_frozen_index_scroll_without_pinning_columns() {
    for index in [-1, 0, 1] {
        let (sender, _) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(Arc::new(DemoService), sender, true);
        app.focus = Focus::Content;
        app.path.push(Location {
            id: "db".into(),
            title: "Tasks".into(),
            kind: LocationKind::Database,
            page_id: "parent".into(),
            list_depth: 0,
        });
        let meta = demo_metadata("db");
        let mut view = serde_json::to_value(&meta.views[2]).unwrap();
        view["configuration"]["frozen_column_index"] = serde_json::json!(index);
        let data = demo_data(serde_json::from_value(view).unwrap());
        app.databases.insert(
            "db".into(),
            DatabaseState {
                meta: Some(meta),
                data: Some(data),
                view_index: 2,
                table_cell: 3,
                ..Default::default()
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal.draw(|f| Ui::default().draw(f, &app)).unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.contains("URL"), "frozen index {index}: {text}");
        assert_eq!(
            text.contains("Name"),
            index == 1,
            "frozen index {index}: {text}"
        );
        assert_eq!(
            text.contains("Alpha"),
            index == 1,
            "frozen index {index}: {text}"
        );
    }
}

#[test]
fn narrow_tables_release_frozen_columns_to_keep_properties_reachable() {
    let (sender, _) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(Arc::new(DemoService), sender, true);
    app.focus = Focus::Content;
    app.path.push(Location {
        id: "db".into(),
        title: "Tasks".into(),
        kind: LocationKind::Database,
        page_id: "parent".into(),
        list_depth: 0,
    });
    let meta = demo_metadata("db");
    let mut data = demo_data(meta.views[2].clone());
    data.view
        .configuration
        .as_mut()
        .unwrap()
        .frozen_column_index = Some(2);
    app.databases.insert(
        "db".into(),
        DatabaseState {
            meta: Some(meta),
            view_index: 2,
            data: Some(data),
            table_cell: 3,
            ..Default::default()
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(24, 15)).unwrap();
    let mut ui = Ui::default();
    terminal.draw(|f| ui.draw(f, &app)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect::<String>();
    assert!(text.contains("URL"));
    assert!(!ui.links.is_empty());
}

#[test]
fn screenshot_layouts_show_workflow_columns_and_the_checkbox_property() {
    for layout in ["board", "table"] {
        let mut data =
            demo_data(demo_metadata("db").views[if layout == "board" { 1 } else { 2 }].clone());
        if layout == "board" {
            data.view.configuration = Some(serde_json::from_value(serde_json::json!({
                "type":"board", "group_by":{"type":"status","property_id":"tG`_","group_by":"option","sort":{"type":"ascending"}},
                "properties":[{"property_id":"tG`_","visible":false},{"property_id":"`Mn;","visible":true}]
            })).unwrap());
            data.schema = serde_json::json!({"properties":{
                "Name":{"id":"title","type":"title"},
                "Assign":{"id":"%60Mn%3B","type":"people"},
                "Status":{"id":"tG%60_","type":"status","status":{"options":[
                    {"id":"todo","name":"Not started"},{"id":"doing","name":"In progress"},{"id":"done","name":"Done"}
                ]}}
            }});
        } else {
            data.view.configuration = Some(
                serde_json::from_value(serde_json::json!({
                    "type":"table", "properties":[
                        {"property_id":"title","visible":true,"width":280},
                        {"property_id":"jdbA","visible":true,"width":200},
                        {"property_id":"g?Nq","visible":true,"width":112}
                    ]
                }))
                .unwrap(),
            );
            data.schema = serde_json::json!({"properties":{
                "Name":{"id":"title","type":"title"},"Text":{"id":"jdbA","type":"rich_text"},
                "Checkbox":{"id":"g%3FNq","type":"checkbox"}
            }});
        }
        for (i, row) in data.rows.iter_mut().enumerate() {
            let name = if layout == "board" {
                format!("card {}", i + 1)
            } else {
                format!("{}1", char::from(b'A' + i as u8))
            };
            row.title = name.clone();
            row.page["properties"] = if layout == "board" {
                serde_json::json!({"Name":{"id":"title","type":"title","title":[{"plain_text":name}]},
                    "Status":{"id":"tG%60_","type":"status","status":{"id":(["todo","doing","done"][i]),"name":(["Not started","In progress","Done"][i])}},
                    "Assign":{"id":"%60Mn%3B","type":"people","people":[]}})
            } else {
                serde_json::json!({"Name":{"id":"title","type":"title","title":[{"plain_text":name}]},
                    "Text":{"id":"jdbA","type":"rich_text","rich_text":[{"plain_text":format!("{}2", char::from(b'A' + i as u8))}]},
                    "Checkbox":{"id":"g%3FNq","type":"checkbox","checkbox":i==0}})
            };
        }
        // Unsorted API queries return newest first; rendering must keep that
        // order while putting board cards into their workflow columns.
        data.rows.reverse();
        let db = DatabaseState {
            data: Some(data.clone()),
            ..Default::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 18)).unwrap();
        let mut ui = Ui::default();
        terminal
            .draw(|f| {
                if layout == "board" {
                    ui.database_board(f, &data, &db, f.area(), "board");
                } else {
                    ui.database_table(f, &data, &db, f.area(), "table");
                }
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let screen = buffer
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        if layout == "board" {
            for (x, label, card) in [
                (0, "Not started (1)", "card 1"),
                (30, "In progress (1)", "card 2"),
                (60, "Done (1)", "card 3"),
            ] {
                let column = (0..18)
                    .flat_map(|y| (x..x + 30).map(move |x| buffer[(x, y)].symbol()))
                    .collect::<String>();
                assert!(column.contains(label), "{column}");
                assert!(column.contains(card), "{column}");
            }
            assert!(!screen.contains("No value"));
            assert!(!screen.contains("Assign:"));
        } else {
            assert!(screen.contains("Checkbox"), "{screen}");
            assert!(screen.contains("☑"));
            assert!(screen.contains("☐"));
            assert!(screen.find("C1").unwrap() < screen.find("B1").unwrap());
            assert!(screen.find("B1").unwrap() < screen.find("A1").unwrap());
        }
    }
}
