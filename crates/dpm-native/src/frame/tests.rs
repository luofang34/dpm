use super::*;
use std::io::{BufReader, Cursor, Read};

fn reader(bytes: &[u8], limit: usize, capacity: usize) -> FrameReader<BufReader<Cursor<Vec<u8>>>> {
    FrameReader::new(
        BufReader::with_capacity(capacity, Cursor::new(bytes.to_vec())),
        limit,
    )
}

fn complete(text: &str) -> Next {
    Next::Frame(Frame::Complete(text.into()))
}

#[test]
fn frames_are_read_in_order_and_the_end_is_distinguished() {
    let mut frames = reader(b"one\ntwo\n\nthree\n", 64, 4);
    for expected in ["one", "two", "", "three"] {
        assert_eq!(frames.next_blocking().expect("read"), complete(expected));
    }
    assert_eq!(frames.next_blocking().expect("read"), Next::End);
    assert_eq!(frames.next_blocking().expect("read"), Next::End);
}

#[test]
fn a_frame_of_exactly_the_limit_is_accepted_and_one_more_byte_is_refused() {
    let mut frames = reader(b"12345\n123456\nok\n", 5, 3);
    assert_eq!(frames.next_blocking().expect("read"), complete("12345"));
    assert_eq!(
        frames.next_blocking().expect("read"),
        Next::Frame(Frame::Rejected(FrameFault::TooLarge { limit: 5 }))
    );
    // The refused frame was consumed through its newline, so the next one is read in step.
    assert_eq!(frames.next_blocking().expect("read"), complete("ok"));
}

#[test]
fn an_oversized_frame_is_discarded_as_it_streams_and_never_buffered() {
    // 64 MiB of one line against a 1 KiB limit, delivered through a tiny buffer.
    let endless = io::repeat(b'a').take(64 * 1024 * 1024);
    let input = endless.chain(Cursor::new(b"\nafter\n".to_vec()));
    let mut frames = FrameReader::new(BufReader::with_capacity(8192, input), 1024);
    assert_eq!(
        frames.next_blocking().expect("read"),
        Next::Frame(Frame::Rejected(FrameFault::TooLarge { limit: 1024 }))
    );
    assert!(
        frames.high_water() <= 1024,
        "held {} bytes for one frame",
        frames.high_water()
    );
    assert_eq!(frames.next_blocking().expect("read"), complete("after"));
}

#[test]
fn endless_input_without_a_newline_ends_as_truncated_with_bounded_memory() {
    let input = io::repeat(b'x').take(8 * 1024 * 1024);
    let mut frames = FrameReader::new(BufReader::with_capacity(4096, input), 512);
    assert_eq!(frames.next_blocking().expect("read"), Next::Truncated);
    assert!(frames.high_water() <= 512);
}

#[test]
fn text_that_is_not_utf8_is_refused_and_the_stream_goes_on() {
    let mut frames = reader(b"\xff\xfe broken\n{\"ok\":true}\n", 64, 5);
    assert_eq!(
        frames.next_blocking().expect("read"),
        Next::Frame(Frame::Rejected(FrameFault::NotUtf8))
    );
    assert_eq!(
        frames.next_blocking().expect("read"),
        complete("{\"ok\":true}")
    );
}

#[test]
fn input_that_ends_inside_a_frame_is_truncated_never_a_short_frame() {
    let mut frames = reader(b"whole\npartial", 64, 4);
    assert_eq!(frames.next_blocking().expect("read"), complete("whole"));
    assert_eq!(frames.next_blocking().expect("read"), Next::Truncated);
    assert_eq!(frames.next_blocking().expect("read"), Next::End);
}

#[test]
fn a_read_failure_is_reported_not_mistaken_for_the_end() {
    struct Failing;
    impl io::Read for Failing {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"))
        }
    }
    let mut frames = FrameReader::new(BufReader::new(Failing), 64);
    assert_eq!(
        frames.next_blocking().expect_err("a failure").kind(),
        io::ErrorKind::BrokenPipe
    );
}
