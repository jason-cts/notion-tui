use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer as Screen};

fn content_app() -> (
    App,
    tokio::sync::mpsc::UnboundedReceiver<crate::app::Message>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(std::sync::Arc::new(crate::service::DemoService), tx, true);
    app.path.push(crate::app::Location {
        id: "page".into(),
        title: "Page".into(),
        kind: crate::app::LocationKind::Page,
        page_id: "page".into(),
        list_depth: 0,
    });
    app.focus = Focus::Content;
    app.blocks.push(crate::model::tests::paragraph());
    (app, rx)
}

fn draw(app: &App, width: u16, height: u16) -> (Screen, Ui) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut ui = Ui::default();
    terminal.draw(|frame| ui.draw(frame, app)).unwrap();
    (terminal.backend().buffer().clone(), ui)
}

fn text(screen: &Screen) -> String {
    screen.content.iter().map(|cell| cell.symbol()).collect()
}

async fn demo_modal() -> App {
    let (mut app, mut rx) = content_app();
    app.blocks[0].id = "demo-1".into();
    app.key(KeyCode::Char('c').into());
    app.receive(rx.recv().await.unwrap());
    app
}

#[tokio::test]
async fn balloon_is_right_aligned_once_and_preserves_wrapping_selection_and_links() {
    let mut app = demo_modal().await;
    app.key(KeyCode::Esc.into());
    app.blocks[0] = crate::model::BlockItem::from_value(&serde_json::json!({
        "id":"demo-1","type":"paragraph","paragraph":{"rich_text":[
            {"type":"text","text":{"content":"你好 café ".repeat(10)}},
            {"type":"text","text":{"content":"Open link","link":{"url":"https://example.com/"}},"href":"https://example.com/"}
        ]}
    }));
    for (width, height) in [(120, 25), (79, 25), (35, 25), (16, 10)] {
        let (screen, ui) = draw(&app, width, height);
        let positions: Vec<_> = screen
            .content
            .iter()
            .enumerate()
            .filter(|(_, c)| c.symbol() == BALLOON)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions.len(), 1, "{width}x{height}");
        let position = positions[0];
        assert_eq!(position % usize::from(width), usize::from(width) - 3);
        let balloon = &screen.content[position];
        assert_eq!(balloon.bg, Color::Rgb(43, 53, 66));
        assert_eq!(ui.visible_blocks, ["demo-1"]);
        // Removing the known presence must not reflow any content or links.
        let mut unknown = content_app().0;
        unknown.blocks = app.blocks.clone();
        let (without, other_ui) = draw(&unknown, width, height);
        for y in 3..height.saturating_sub(3) {
            for x in if width >= 80 { width * 30 / 100 + 1 } else { 1 }..width.saturating_sub(3) {
                assert_eq!(screen[(x, y)].symbol(), without[(x, y)].symbol());
            }
        }
        assert_eq!(ui.links, other_ui.links);
    }
}

#[tokio::test]
async fn comments_compose_confirm_and_long_unicode_render_at_all_sizes() {
    let mut app = demo_modal().await;
    let modal = app.comments.as_mut().unwrap();
    modal.items[0].runs = vec![RichRun::plain("你好 👋 café\n".repeat(40))];
    for (width, height) in [(120, 32), (45, 16), (20, 10), (12, 5), (1, 1)] {
        let (screen, ui) = draw(&app, width, height);
        assert!(ui.links.is_empty());
        if width >= 20 {
            assert!(text(&screen).contains("COMMENTS"));
        }
        app.key(KeyCode::PageDown.into());
        draw(&app, width, height);
        app.key(KeyCode::PageUp.into());
        app.key(KeyCode::Char('a').into());
        app.paste("你好\nrich draft");
        app.key(KeyEvent::new(KeyCode::Home, KeyModifiers::SHIFT));
        app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        draw(&app, width, height);
        app.key(KeyEvent::new(KeyCode::F(10), KeyModifiers::SHIFT));
        draw(&app, width, height);
        app.key(KeyCode::Esc.into());
        app.key(KeyCode::Esc.into());
        draw(&app, width, height);
        app.key(KeyCode::Char('y').into());
        app.key(KeyCode::Char('d').into());
        draw(&app, width, height);
        app.key(KeyCode::Esc.into());
    }
}

#[tokio::test]
async fn comments_show_empty_loading_errors_and_threads_with_attachment_indicators() {
    let mut app = demo_modal().await;
    app.comments.as_mut().unwrap().items[1].discussion = "other".into();
    app.comments.as_mut().unwrap().items[1].attachments = 2;
    let (screen, _) = draw(&app, 100, 30);
    let rendered = text(&screen);
    assert!(rendered.contains("Discussion 1"));
    assert!(rendered.contains("Discussion 2"));
    assert!(rendered.contains("[2 attachments]"));
    app.comments.as_mut().unwrap().loading = true;
    assert!(text(&draw(&app, 100, 30).0).contains("Loading comments"));
    app.comments.as_mut().unwrap().loading = false;
    app.comments.as_mut().unwrap().error = Some("Read comments access denied".into());
    assert!(text(&draw(&app, 100, 30).0).contains("Read comments access denied"));
    app.comments.as_mut().unwrap().error = None;
    app.comments.as_mut().unwrap().items.clear();
    assert!(text(&draw(&app, 100, 30).0).contains("No comments yet"));
}

#[tokio::test]
async fn renderer_reports_only_visible_blocks_and_retains_scroll_after_modal_closes() {
    let mut app = demo_modal().await;
    app.key(KeyCode::Esc.into());
    let original = app.blocks[0].clone();
    app.blocks = (0..20)
        .map(|i| {
            let mut block = original.clone();
            block.id = format!("block-{i}");
            block
        })
        .collect();
    app.block_selected = 12;
    let mut terminal = Terminal::new(TestBackend::new(60, 16)).unwrap();
    let mut ui = Ui::default();
    terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
    let visible = ui.visible_blocks.clone();
    assert!(visible.contains(&"block-12".into()));
    assert!(!visible.contains(&"block-0".into()));
    app.key(KeyCode::Char('c').into());
    terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
    app.key(KeyCode::Esc.into());
    terminal.draw(|frame| ui.draw(frame, &app)).unwrap();
    assert_eq!(ui.visible_blocks, visible);
    assert_eq!(app.block_selected, 12);
}
