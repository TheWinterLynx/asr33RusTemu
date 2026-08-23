use asr33emu::core::terminal::{
    EscapeFilter, Terminal, TerminalOptions, encode_even_parity, is_legacy_printable_ascii,
    mask_parity_bit,
};

fn terminal(columns: usize, rows: usize, scrollback: usize, autowrap: bool) -> Terminal {
    Terminal::new(TerminalOptions {
        columns,
        rows,
        scrollback,
        autowrap,
    })
    .expect("test terminal options are valid")
}

fn line_text(terminal: &Terminal, row: usize) -> String {
    terminal
        .line_history()
        .line(row)
        .map(|line| line.top_characters())
        .expect("test row exists")
}

#[test]
fn cursor_column_and_width_are_the_single_status_counter_source() {
    let mut state = terminal(72, 4, 0, false);
    assert_eq!(state.cursor_position().0, 0);
    assert_eq!(state.width(), 72);
    state.receive_data(b"ABC").expect("text accepted");
    assert_eq!(state.cursor_position().0, 3);
    state.receive_data(b"\n").expect("LF accepted");
    assert_eq!(state.cursor_position().0, 3, "LF preserves column");
    state.receive_data(b"\r\n").expect("CRLF accepted");
    assert_eq!(state.cursor_position().0, 0);

    let mut wrapping = terminal(4, 2, 0, true);
    wrapping.receive_data(b"ABCDE").expect("autowrap accepted");
    assert_eq!(wrapping.cursor_position(), (1, 1));
}

#[test]
fn parity_examples_match_python() {
    assert_eq!(encode_even_parity(b"AC\x7f"), b"A\xc3\xff");
    assert_eq!(mask_parity_bit(b"\x00\x7f\x80\xff"), b"\x00\x7f\x00\x7f");
}

#[test]
fn printable_fixture_matches_rust_for_all_seven_bit_values() {
    let fixture = include_str!("fixtures/ascii_7bit_printable.csv");
    let mut observed = Vec::new();
    for line in fixture.lines().skip(1) {
        let Some((byte, printable)) = line.split_once(',') else {
            panic!("invalid fixture row: {line}");
        };
        let byte: u8 = byte.parse().expect("fixture byte is a valid u8");
        let expected = match printable {
            "true" => true,
            "false" => false,
            value => panic!("invalid printable value: {value}"),
        };
        observed.push(byte);
        assert_eq!(
            is_legacy_printable_ascii(byte),
            expected,
            "byte {byte:#04x}"
        );
    }
    assert_eq!(observed, (0_u8..=0x7f).collect::<Vec<_>>());
}

#[test]
fn escape_filter_examples_match_python() {
    let mut filter = EscapeFilter::new();
    let output: Vec<u8> = [
        b"A\x1b[31".as_slice(),
        b"mB\x1b]title".as_slice(),
        b"\x07C\x1b]other\x1b".as_slice(),
        b"\\D".as_slice(),
    ]
    .into_iter()
    .flat_map(|chunk| filter.feed(chunk))
    .collect();
    assert_eq!(output, b"ABCD");
    assert_eq!(EscapeFilter::new().feed(b"A\x1b7B"), b"AB");
}

#[test]
fn control_character_cursor_rules_match_python() {
    let mut terminal = terminal(16, 2, 2, false);
    terminal
        .receive_data(b"AB\x08X\x09Y\x0cZ\rQ\nR\x0bS")
        .expect("logical line does not overflow");
    assert_eq!(line_text(&terminal, 0), "QX      Z       ");
    assert_eq!(line_text(&terminal, 1), " R              ");
    assert_eq!(line_text(&terminal, 2), "  S             ");
    assert_eq!(terminal.cursor_position(), (3, 2));
}

#[test]
fn form_feed_is_a_one_column_backspace() {
    let mut terminal = terminal(8, 2, 2, false);
    terminal
        .receive_data(b"AB\x0cX")
        .expect("logical line does not overflow");
    assert_eq!(
        terminal
            .line_history()
            .line(0)
            .expect("first line exists")
            .strike_stack(1),
        ['B', 'X']
    );
}

#[test]
fn disabled_autowrap_overstrikes_last_column() {
    let mut terminal = terminal(4, 2, 2, false);
    terminal
        .receive_data(b"ABCDE")
        .expect("logical line does not overflow");
    assert_eq!(line_text(&terminal, 0), "ABCE");
    assert_eq!(
        terminal
            .line_history()
            .line(0)
            .expect("first line exists")
            .strike_stack(3),
        ['D', 'E']
    );
    assert_eq!(terminal.cursor_position(), (3, 0));
}

#[test]
fn enabled_autowrap_starts_new_logical_line() {
    let mut terminal = terminal(4, 2, 2, true);
    terminal
        .receive_data(b"ABCDE")
        .expect("logical line does not overflow");
    assert_eq!(line_text(&terminal, 0), "ABCD");
    assert_eq!(line_text(&terminal, 1), "E   ");
    assert_eq!(terminal.cursor_position(), (1, 1));
}

#[test]
fn scrollback_and_logical_numbers_match_python() {
    let mut terminal = terminal(8, 2, 1, false);
    terminal
        .receive_data(b"0\n1\n2\n3")
        .expect("logical line does not overflow");
    assert_eq!(terminal.line_history().len(), 3);
    assert_eq!(terminal.line_history().top_logical_number(), Some(1));
    assert_eq!(terminal.line_history().bottom_logical_number(), Some(3));
    assert_eq!(
        (0..3)
            .map(|row| line_text(&terminal, row))
            .collect::<Vec<_>>(),
        [" 1      ", "  2     ", "   3    "]
    );
}

#[test]
fn remote_msb_is_masked_but_forwarded_bytes_are_original() {
    let mut terminal = terminal(8, 2, 2, false);
    let effects = terminal
        .receive_data(b"\xc1")
        .expect("logical line does not overflow");
    assert_eq!(line_text(&terminal, 0), "A       ");
    assert_eq!(effects.forwarded_bytes, b"\xc1");
}

#[test]
fn printer_off_forwards_without_rendering_or_events() {
    let mut terminal = terminal(8, 2, 2, false);
    terminal.disable_printing();
    let effects = terminal
        .receive_data(b"A")
        .expect("logical line does not overflow");
    assert_eq!(line_text(&terminal, 0), "        ");
    assert_eq!(terminal.character_event_count(), 0);
    assert_eq!(effects.forwarded_bytes, b"A");
}

#[test]
fn character_events_include_controls_with_post_processing_column() {
    let mut terminal = terminal(8, 2, 2, false);
    terminal
        .receive_data(b"A\r\n\x01")
        .expect("logical line does not overflow");
    let mut events = Vec::new();
    while let Some(event) = terminal.pop_character_event() {
        events.push((event.character, event.column));
    }
    assert_eq!(events, [('A', 1), ('\r', 0), ('\n', 0), ('\x01', 0)]);
}
