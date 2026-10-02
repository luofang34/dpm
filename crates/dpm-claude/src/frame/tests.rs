use super::*;
use std::io::Cursor;

fn frames(input: &[u8], limit: usize) -> Vec<Frame> {
    let mut reader = LineReader::new(Cursor::new(input.to_vec()), limit);
    let mut found = Vec::new();
    while let Some(frame) = reader.next_blocking().expect("read") {
        found.push(frame);
    }
    found
}

#[test]
fn lines_are_returned_in_order_and_a_clean_end_is_not_a_frame() {
    assert_eq!(
        frames(b"one\ntwo\n\nthree\n", 64),
        vec![
            Frame::Line("one".into()),
            Frame::Line("two".into()),
            Frame::Line(String::new()),
            Frame::Line("three".into()),
        ]
    );
}

#[test]
fn a_line_over_the_bound_is_counted_and_discarded_and_the_next_line_survives() {
    let mut input = vec![b'x'; 1000];
    input.extend_from_slice(b"\nok\n");
    assert_eq!(
        frames(&input, 100),
        vec![Frame::Oversized { bytes: 1000 }, Frame::Line("ok".into())]
    );
}

#[test]
fn a_line_of_exactly_the_bound_is_kept_and_one_more_byte_is_not() {
    assert_eq!(
        frames(b"12345\n123456\n", 5),
        vec![Frame::Line("12345".into()), Frame::Oversized { bytes: 6 }]
    );
}

#[test]
fn input_that_ends_inside_a_line_is_truncated_never_a_short_line() {
    assert_eq!(
        frames(b"whole\npart", 64),
        vec![Frame::Line("whole".into()), Frame::Truncated { bytes: 4 }]
    );
}

#[test]
fn text_that_is_not_utf8_is_reported_and_reading_continues() {
    assert_eq!(
        frames(b"\xff\xfe bad\nfine\n", 64),
        vec![Frame::NotUtf8 { bytes: 6 }, Frame::Line("fine".into())]
    );
}

#[test]
fn endless_input_without_a_newline_never_grows_the_buffer() {
    struct Endless(usize);
    impl std::io::Read for Endless {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Ok(0);
            }
            let count = buffer.len().min(self.0);
            buffer.iter_mut().take(count).for_each(|byte| *byte = b'z');
            self.0 -= count;
            Ok(count)
        }
    }
    let mut reader = LineReader::new(
        std::io::BufReader::with_capacity(4096, Endless(10_000_000)),
        1024,
    );
    assert_eq!(
        reader.next_blocking().expect("read"),
        Some(Frame::Truncated { bytes: 10_000_000 })
    );
}
