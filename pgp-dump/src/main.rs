use color_eyre::eyre::Result;
use crossterm::event::KeyCode;
use ratatui::{prelude::*, widgets::*};
use tokio::sync::mpsc;
use tui_tree_widget::{Tree, TreeItem, TreeState};

pub fn initialize_panic_handler() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        shutdown().unwrap();
        original_hook(panic_info);
    }));
}

fn startup() -> Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(std::io::stderr(), crossterm::terminal::EnterAlternateScreen)?;
    Ok(())
}

fn shutdown() -> Result<()> {
    crossterm::execute!(std::io::stderr(), crossterm::terminal::LeaveAlternateScreen)?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

struct App<'a> {
    action_tx: mpsc::UnboundedSender<Action>,
    should_quit: bool,
    state: TreeState<usize>,
    items: Vec<TreeItem<'a, usize>>,
    packets: Vec<pgp::packet::Packet>,
}

impl App<'_> {
    fn new(action_tx: mpsc::UnboundedSender<Action>, packets: Vec<pgp::packet::Packet>) -> Self {
        let mut items = Vec::new();

        for (i, packet) in packets.iter().enumerate() {
            let name = format!("{:?}", packet.tag());
            items.push(TreeItem::new_leaf(i, name));
        }

        Self {
            should_quit: false,
            action_tx,
            state: TreeState::default(),
            items,
            packets,
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let area = f.size();
        let layout = Layout::default()
            .direction(Direction::Horizontal)
            // use a 49/51 split instead of 50/50 to ensure that any extra space is on the right
            // side of the screen. This is important because the right side of the screen is
            // where the borders are collapsed.
            .constraints([Constraint::Percentage(49), Constraint::Percentage(51)])
            .split(area);

        let widget = Tree::new(self.items.clone())
            .expect("all item identifiers are unique")
            .block(
                Block::new()
                    .title("Packets")
                    .title_bottom(format!("{:?}", self.state))
                    // don't render the right border because it will be rendered by the right block
                    .border_set(symbols::border::PLAIN)
                    .borders(Borders::TOP | Borders::LEFT | Borders::BOTTOM),
            )
            .experimental_scrollbar(Some(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .track_symbol(None)
                    .end_symbol(None),
            ))
            .highlight_style(
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">> ");
        f.render_stateful_widget(widget, layout[0], &mut self.state);

        let text = if let Some(i) = self.state.selected().last() {
            format!("{:#?}", self.packets[*i])
        } else {
            "Nothing selected".to_string()
        };

        f.render_widget(
            Paragraph::new(text).block(
                Block::new()
                    // don't render the right border because it will be rendered by the right block
                    .border_set(symbols::border::PLAIN)
                    .borders(Borders::TOP | Borders::LEFT | Borders::BOTTOM | Borders::RIGHT)
                    .title("Details"),
            ),
            layout[1],
        );
    }

    fn update(&mut self, msg: Action) -> Action {
        match msg {
            Action::Quit => self.should_quit = true, // You can handle cleanup and exit here
            Action::Up => {
                self.state.key_up(&self.items);
            }
            Action::Down => {
                self.state.key_down(&self.items);
            }
            Action::Left => {
                self.state.key_left();
            }
            Action::Right => {
                self.state.key_right();
            }
            Action::None => {}
        };
        Action::None
    }

    fn handle_event(&self) -> tokio::task::JoinHandle<()> {
        let tick_rate = std::time::Duration::from_millis(250);
        let tx = self.action_tx.clone();
        tokio::spawn(async move {
            loop {
                let action = if crossterm::event::poll(tick_rate).unwrap() {
                    if let crossterm::event::Event::Key(key) = crossterm::event::read().unwrap() {
                        if key.kind == crossterm::event::KeyEventKind::Press {
                            match key.code {
                                KeyCode::Char('q') => Action::Quit,
                                KeyCode::Left => Action::Left,
                                KeyCode::Right => Action::Right,
                                KeyCode::Down => Action::Down,
                                KeyCode::Up => Action::Up,
                                _ => Action::None,
                            }
                        } else {
                            Action::None
                        }
                    } else {
                        Action::None
                    }
                } else {
                    Action::None
                };
                if let Err(_) = tx.send(action) {
                    break;
                }
            }
        })
    }
}

#[derive(PartialEq)]
enum Action {
    Left,
    Right,
    Down,
    Up,
    Quit,
    None,
}

async fn run(packets: Vec<pgp::packet::Packet>) -> Result<()> {
    let mut t = Terminal::new(CrosstermBackend::new(std::io::stderr()))?;

    let (action_tx, mut action_rx) = mpsc::unbounded_channel();

    let mut app = App::new(action_tx, packets);
    let task = app.handle_event();

    loop {
        t.draw(|f| {
            app.draw(f);
        })?;

        if let Some(action) = action_rx.recv().await {
            app.update(action);
        }

        if app.should_quit {
            break;
        }
    }

    task.abort();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    initialize_panic_handler();

    let file = std::env::args().nth(1).expect("missing file");
    let file = tokio::fs::read_to_string(file).await?;

    let mut dearmor = pgp::armor::Dearmor::new(file.as_bytes());
    dearmor.read_header()?;
    let packets = pgp::packet::PacketParser::new(dearmor).collect::<Result<_, _>>()?;

    startup()?;
    run(packets).await?;
    shutdown()?;
    Ok(())
}
