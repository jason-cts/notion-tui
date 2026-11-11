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
                horizontal: 1,
                ..Default::default()
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
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
            horizontal: 2,
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
