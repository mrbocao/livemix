use crate::egui;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Panel {
    busy: bool,
    address: String,
    status: String,
    clips: Vec<(u32, String)>,
    clip: u32,
    count: u32,
    last_poll: Option<std::time::Instant>,
    transport: String,
    playlist_running: bool,
    stop_playlist: bool,
}

fn line(reader: &mut impl BufRead) -> Result<String, String> {
    let mut line = String::new();
    reader.take(8192).read_line(&mut line).map_err(|error| error.to_string())?;
    if !line.ends_with('\n') { return Err("Incomplete HyperDeck response".into()); }
    Ok(line.trim_end().to_owned())
}

fn response(reader: &mut impl BufRead) -> Result<(u16, Vec<String>), String> {
    let header = line(reader)?;
    let code = header.split_once(' ').and_then(|(code, _)| code.parse().ok())
        .ok_or("Invalid HyperDeck response")?;
    let mut fields = Vec::new();
    if header.ends_with(':') {
        loop {
            let field = line(reader)?;
            if field.is_empty() { break; }
            if fields.len() >= 2048 { return Err("HyperDeck response exceeds 2048 rows".into()); }
            fields.push(field);
        }
    }
    if (100..200).contains(&code) { return Err(header); }
    Ok((code, fields))
}

fn connect(address: SocketAddr) -> Result<BufReader<TcpStream>, String> {
    let timeout = Duration::from_secs(2);
    let stream = TcpStream::connect_timeout(&address, timeout).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    if response(&mut reader)?.0 != 500 { return Err("HyperDeck connection banner missing".into()); }
    Ok(reader)
}

fn command(reader: &mut BufReader<TcpStream>, command: &str) -> Result<Vec<String>, String> {
    writeln!(reader.get_mut(), "{command}").map_err(|e| e.to_string())?;
    for _ in 0..16 {
        let (code, result) = response(reader)?;
        if (200..300).contains(&code) { return Ok(result); }
        if !(500..600).contains(&code) { return Err(format!("Unexpected HyperDeck response {code}")); }
    }
    Err("HyperDeck returned too many unsolicited messages".into())
}

fn exchange(address: SocketAddr, commands: &[String]) -> Result<Vec<String>, String> {
    let mut connection = connect(address)?;
    let mut fields = Vec::new();
    for request in commands { fields = command(&mut connection, request)?; }
    Ok(fields)
}

fn playlist_ids(value: &str) -> Option<Vec<u32>> {
    let ids = value.split(',').map(|item| item.trim().parse::<u32>().ok().filter(|&n| n > 0))
        .collect::<Option<Vec<_>>>()?;
    (ids.len() <= 128).then_some(ids)
}

fn play_playlist(address: SocketAddr, ids: &[u32], panel: &Mutex<Panel>) -> Result<&'static str, String> {
    let mut connection = connect(address)?;
    command(&mut connection, "playrange clear")?;
    for (position, clip) in ids.iter().enumerate() {
        if panel.lock().unwrap().stop_playlist {
            command(&mut connection, "stop")?;
            return Ok("Playlist stopped");
        }
        command(&mut connection, &format!("play: clip id: {clip} single clip: true loop: false"))?;
        panel.lock().unwrap().status = format!("Playlist {}/{} · Clip {clip}", position + 1, ids.len());
        let confirmation = std::time::Instant::now() + Duration::from_secs(3);
        let mut observed = false;
        loop {
            if panel.lock().unwrap().stop_playlist {
                command(&mut connection, "stop")?;
                return Ok("Playlist stopped");
            }
            let fields = command(&mut connection, "transport info")?;
            let status = fields.iter().find_map(|field| field.strip_prefix("status: "));
            let current = fields.iter().find_map(|field| field.strip_prefix("clip id: "))
                .and_then(|id| id.parse::<u32>().ok());
            match status {
                Some("play") if current == Some(*clip) => observed = true,
                Some("stopped") if observed => break,
                Some("record") => return Err("HyperDeck began recording during the playlist".into()),
                _ if !observed && std::time::Instant::now() >= confirmation =>
                    return Err(format!("HyperDeck did not confirm playback of clip {clip}")),
                _ if observed => return Err("HyperDeck transport changed during the playlist".into()),
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }
    Ok("Playlist finished")
}

fn clips(fields: Vec<String>) -> Vec<(u32, String)> {
    fields.into_iter().filter_map(|field| {
        let (number, tail) = field.split_once(':')?;
        let id = number.parse().ok()?;
        let mut words = tail.trim().rsplitn(3, ' ');
        words.next()?;
        words.next()?;
        Some((id, words.next()?.to_owned()))
    }).collect()
}

pub fn show(ui: &mut egui::Ui, id: u32, address: &mut String, playlist: &mut String) {
    ui.collapsing("Blackmagic HyperDeck", |ui| {
        ui.label("HyperDeck IP address");
        ui.add(egui::TextEdit::singleline(address).char_limit(45));
        let key = ui.make_persistent_id(("hyperdeck", id));
        let state = ui.data_mut(|data| data.get_temp_mut_or_insert_with(key, || Arc::new(Mutex::new(Panel {
            clip: 1, count: 1, ..Default::default()
        }))).clone());
        let mut panel = state.lock().unwrap();
        if !panel.busy && panel.address != *address {
            panel.address.clone_from(address);
            panel.status.clear();
            panel.clips.clear();
            panel.transport.clear();
            panel.last_poll = None;
        }
        let ip = address.trim().parse::<IpAddr>().ok();
        let mut commands = None;
        ui.add_enabled_ui(ip.is_some() && !panel.busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("List clips").clicked() { commands = Some(vec!["clips get".into()]); }
                if ui.button("Record on HyperDeck").clicked() { commands = Some(vec!["record".into()]); }
                if ui.button("Stop HyperDeck").clicked() { commands = Some(vec!["stop".into()]); }
            });
            let mut selected = panel.clip;
            egui::ComboBox::from_id_salt("hyperdeck-clips").selected_text(format!("Clip {}", panel.clip)).show_ui(ui, |ui| {
                for (id, name) in &panel.clips { ui.selectable_value(&mut selected, *id, format!("{id}: {name}")); }
            });
            panel.clip = selected;
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::DragValue::new(&mut panel.clip).range(1..=999_999).prefix("Clip "));
                if ui.button("Play clip").clicked() {
                    commands = Some(vec!["playrange clear".into(),
                        format!("play: clip id: {} single clip: true", panel.clip)]);
                }
                ui.add(egui::DragValue::new(&mut panel.count).range(1..=999).suffix(" clips"));
                if ui.button("Play range").clicked() {
                    commands = Some(vec![format!("playrange set: clip id: {} count: {}", panel.clip, panel.count),
                        "play: single clip: false".into()]);
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Playlist clip IDs");
            ui.add(egui::TextEdit::singleline(playlist).char_limit(1024).hint_text("3, 1, 8"));
            let ids = playlist_ids(playlist);
            if ui.add_enabled(ip.is_some() && !panel.busy && ids.is_some(), egui::Button::new("Play playlist")).clicked() {
                let socket = SocketAddr::new(ip.unwrap(), 9993);
                let ids = ids.unwrap();
                panel.busy = true;
                panel.playlist_running = true;
                panel.stop_playlist = false;
                panel.status = "Starting HyperDeck playlist…".into();
                let state = state.clone();
                std::thread::spawn(move || {
                    let result = play_playlist(socket, &ids, &state);
                    let mut panel = state.lock().unwrap();
                    panel.busy = false;
                    panel.playlist_running = false;
                    panel.status = result.map_or_else(|error| error, str::to_owned);
                });
            }
            if ui.add_enabled(panel.playlist_running, egui::Button::new("Stop playlist")).clicked() {
                panel.stop_playlist = true;
            }
        });
        if panel.busy { ui.ctx().request_repaint_after(Duration::from_millis(100)); }
        ui.label(&panel.status);
        if !panel.transport.is_empty() { ui.label(&panel.transport); }
        if ip.is_some() && !panel.busy && commands.is_none() {
            if panel.last_poll.is_none_or(|at| at.elapsed() >= Duration::from_secs(2)) {
                commands = Some(vec!["transport info".into()]);
            } else { ui.ctx().request_repaint_after(Duration::from_secs(2)); }
        }
        if let (Some(ip), Some(commands)) = (ip, commands) {
            panel.last_poll = Some(std::time::Instant::now());
            panel.busy = true;
            if commands[0] != "transport info" { panel.status = "Waiting for HyperDeck…".into(); }
            ui.ctx().request_repaint_after(Duration::from_millis(100));
            let state = state.clone();
            std::thread::spawn(move || {
                let list = commands[0] == "clips get";
                let poll = commands[0] == "transport info";
                let result = exchange(SocketAddr::new(ip, 9993), &commands);
                let mut panel = state.lock().unwrap();
                panel.busy = false;
                if poll {
                    panel.transport = match result {
                        Ok(fields) => fields.into_iter().filter(|field| field.starts_with("status:") || field.starts_with("clip id:"))
                            .collect::<Vec<_>>().join(" · "),
                        Err(error) => error,
                    };
                    return;
                }
                panel.status = match result {
                    Ok(fields) if list => {
                        panel.clips = clips(fields);
                        format!("{} clips", panel.clips.len())
                    }
                    Ok(_) => "HyperDeck accepted command".into(),
                    Err(error) => error,
                };
            });
        }
    });
}

#[test]
fn parses_hyperdeck_clips_with_spaces() {
    let reply = b"205 clips info:\r\nclip count: 1\r\n1: Take One.mov 00:00:00:00 00:00:10:00\r\n\r\n";
    let (code, fields) = response(&mut &reply[..]).unwrap();
    assert_eq!(code, 205);
    assert_eq!(clips(fields), vec![(1, "Take One.mov".into())]);
}

#[test]
fn playlist_preserves_clip_order_and_repeats() {
    assert_eq!(playlist_ids("3, 1, 3"), Some(vec![3, 1, 3]));
    assert_eq!(playlist_ids("3,0"), None);
    assert_eq!(playlist_ids("3\nrecord"), None);
}

#[test]
fn sends_hyperdeck_play_range_over_tcp() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let deck = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        stream.write_all(b"500 connection info:\r\nmodel: Test Deck\r\n\r\n").unwrap();
        let mut reader = BufReader::new(stream);
        assert_eq!(line(&mut reader).unwrap(), "playrange set: clip id: 2 count: 3");
        reader.get_mut().write_all(b"508 transport info:\r\nstatus: stopped\r\n\r\n200 ok\r\n").unwrap();
        assert_eq!(line(&mut reader).unwrap(), "play: single clip: false");
        reader.get_mut().write_all(b"200 ok\r\n").unwrap();
    });
    exchange(address, &["playrange set: clip id: 2 count: 3".into(), "play: single clip: false".into()]).unwrap();
    deck.join().unwrap();
}
