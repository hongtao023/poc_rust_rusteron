use rusteron_bench::driver::EmbeddedDriver;
use rusteron_bench::protocol::{BenchMessage, ControlCode, MsgType, MESSAGE_SIZE};

/// Test that the embedded media driver can be launched and a client connected.
#[test]
fn driver_launch_and_connect() {
    let driver = EmbeddedDriver::launch().expect("Failed to launch embedded driver");
    assert!(!driver.dir.is_empty());
    let _aeron = driver.connect().expect("Failed to connect Aeron client");
    // Leak the driver to prevent Drop from stopping C library background threads
    // which would call exit(1).
    std::mem::forget(driver);
}

/// Test ping-pong echo: send a ping via server, verify pong comes back.
#[test]
fn ping_pong_echo() {
    use rusteron_bench::driver;
    use rusteron_bench::server;
    use rusteron_client::{AeronFragmentHandlerCallback, AeronHeader, Handler};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let drv = EmbeddedDriver::launch().expect("Failed to launch driver");
    let drv_dir = drv.dir.clone();
    let server_stop = Arc::new(AtomicBool::new(false));
    let server_stop_clone = server_stop.clone();

    let endpoint = "localhost:20231";
    let reply_endpoint = "localhost:20232";
    let stream_send = 3001;
    let stream_recv = 3002;

    let ep = endpoint.to_string();
    let rep = reply_endpoint.to_string();

    // Spawn server thread
    let server_handle = std::thread::spawn(move || {
        let server_driver = EmbeddedDriver {
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
            dir: drv_dir,
        };
        server::run_server(
            &server_driver,
            &ep,
            &rep,
            stream_send,
            stream_recv,
            server_stop_clone,
        )
        .expect("Server failed");
    });

    std::thread::sleep(Duration::from_millis(500));

    // Client side
    let aeron = drv.connect().expect("Failed to connect client");
    let publication =
        driver::add_publication(&aeron, endpoint, stream_send).expect("Failed to add pub");
    let subscription =
        driver::add_subscription(&aeron, reply_endpoint, stream_recv).expect("Failed to add sub");

    // Send a ping
    let mut buf = [0u8; MESSAGE_SIZE];
    let ping = BenchMessage::new(MsgType::Ping, 7);
    ping.write_to(&mut buf);
    driver::offer_with_retry(&publication, &buf);

    // Wait for pong
    struct PongState {
        received: bool,
        sequence: u32,
    }
    impl AeronFragmentHandlerCallback for PongState {
        fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
            if buffer.len() >= MESSAGE_SIZE {
                let msg = BenchMessage::read_from(buffer);
                if msg.msg_type == MsgType::Pong {
                    self.sequence = msg.sequence;
                    self.received = true;
                }
            }
        }
    }

    let state = PongState {
        received: false,
        sequence: 0,
    };
    let handler = Handler::leak(state);
    let state_ptr = handler.as_raw() as *mut PongState;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let _ = subscription.poll(Some(&handler), 10);
        let received = unsafe { (*state_ptr).received };
        if received {
            break;
        }
        if Instant::now() >= deadline {
            panic!("Timeout waiting for pong");
        }
        std::hint::spin_loop();
    }

    let seq = unsafe { (*state_ptr).sequence };
    assert_eq!(seq, 7, "Pong should echo back the same sequence number");

    // Stop server but leak the driver to avoid C library exit(1)
    server_stop.store(true, Ordering::SeqCst);
    let _ = server_handle.join();
    std::mem::forget(drv);
}

/// Test throughput counting logic: send data messages, request report.
#[test]
fn throughput_counting() {
    use rusteron_bench::driver;
    use rusteron_bench::server;
    use rusteron_client::{AeronFragmentHandlerCallback, AeronHeader, Handler};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    let drv = EmbeddedDriver::launch().expect("Failed to launch driver");
    let drv_dir = drv.dir.clone();
    let server_stop = Arc::new(AtomicBool::new(false));
    let server_stop_clone = server_stop.clone();

    let endpoint = "localhost:20241";
    let reply_endpoint = "localhost:20242";
    let stream_send = 4001;
    let stream_recv = 4002;

    let ep = endpoint.to_string();
    let rep = reply_endpoint.to_string();

    let server_handle = std::thread::spawn(move || {
        let server_driver = EmbeddedDriver {
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
            dir: drv_dir,
        };
        server::run_server(
            &server_driver,
            &ep,
            &rep,
            stream_send,
            stream_recv,
            server_stop_clone,
        )
        .expect("Server failed");
    });

    std::thread::sleep(Duration::from_millis(500));

    let aeron = drv.connect().expect("Failed to connect client");
    let publication =
        driver::add_publication(&aeron, endpoint, stream_send).expect("Failed to add pub");
    let subscription =
        driver::add_subscription(&aeron, reply_endpoint, stream_recv).expect("Failed to add sub");

    let mut buf = [0u8; MESSAGE_SIZE];

    // Send StartThroughput
    let start_msg = BenchMessage::control(ControlCode::StartThroughput, 0);
    start_msg.write_to(&mut buf);
    driver::offer_with_retry(&publication, &buf);

    // Send exactly 100 Data messages
    let msg_count = 100u64;
    for i in 0..msg_count {
        let data = BenchMessage::new(MsgType::Data, i as u32);
        data.write_to(&mut buf);
        driver::offer_with_retry(&publication, &buf);
    }

    // Send StopThroughput
    let stop_msg = BenchMessage::control(ControlCode::StopThroughput, 0);
    stop_msg.write_to(&mut buf);
    driver::offer_with_retry(&publication, &buf);

    // Wait for messages to be processed
    std::thread::sleep(Duration::from_millis(500));

    // Send ReportRequest
    let report_msg = BenchMessage::control(ControlCode::ReportRequest, 0);
    report_msg.write_to(&mut buf);
    driver::offer_with_retry(&publication, &buf);

    // Wait for ReportResponse
    struct ReportState {
        received: bool,
        count: u64,
    }
    impl AeronFragmentHandlerCallback for ReportState {
        fn handle_aeron_fragment_handler(&mut self, buffer: &[u8], _header: AeronHeader) {
            if buffer.len() >= MESSAGE_SIZE {
                let msg = BenchMessage::read_from(buffer);
                if msg.msg_type == MsgType::Control
                    && msg.control_code == ControlCode::ReportResponse
                {
                    self.count = msg.timestamp_ns;
                    self.received = true;
                }
            }
        }
    }

    let state = ReportState {
        received: false,
        count: 0,
    };
    let handler = Handler::leak(state);
    let state_ptr = handler.as_raw() as *mut ReportState;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let _ = subscription.poll(Some(&handler), 10);
        let received = unsafe { (*state_ptr).received };
        if received {
            break;
        }
        if Instant::now() >= deadline {
            panic!("Timeout waiting for report response");
        }
        std::hint::spin_loop();
    }

    let count = unsafe { (*state_ptr).count };
    assert_eq!(
        count, msg_count,
        "Server should have counted exactly {} data messages, got {}",
        msg_count, count
    );

    // Stop server but leak the driver to avoid C library exit(1)
    server_stop.store(true, Ordering::SeqCst);
    let _ = server_handle.join();
    std::mem::forget(drv);
}
