//! Receiver lifetime and capture-transition contracts. Pending capture workers
//! stand in for WASAPI; these tests never open an audio endpoint or AirPlay socket.
use super::*;

const PAIR: &str = "eafa36aa-9785-54b2-a537-d9ee2a55cf1c";

fn receiver(id: &str, pair: bool) -> Device {
    let mut device = Device::manual("192.0.2.20".parse().unwrap(), 7000);
    device.id = id.into();
    device.name = format!("Continuity {id}");
    device.model = Some("AudioAccessory5,1".into());
    if pair {
        device.txt.insert("tsid".into(), PAIR.into());
    }
    device
}

fn actor() -> Actor {
    let state = Snapshot {
        selected_source_id: "default".into(),
        sources: ["default", "speakers", "headphones"]
            .into_iter()
            .map(|id| SourceView {
                id: id.into(),
                name: id.into(),
                kind: "endpoint",
                is_default: id == "default",
            })
            .collect(),
        ..Snapshot::default()
    };
    let mut actor = Actor::new(
        Arc::new(Mutex::new(state)),
        Arc::new(AtomicBool::new(false)),
    );
    actor.devices = vec![
        receiver("left", true),
        receiver("right", true),
        receiver("solo", false),
    ];
    actor.online = actor
        .devices
        .iter()
        .map(|device| device.id.clone())
        .collect();
    // Manual refresh must coalesce outstanding discovery work. Supplying both
    // pending tasks also keeps Windows CI from enumerating real local devices.
    actor.discovery = Some(tokio::spawn(std::future::pending()));
    actor.source_watch_start = Some(tokio::spawn(std::future::pending()));
    actor
}

fn attach(actor: &mut Actor, id: &str) -> mpsc::Receiver<MediaBlock> {
    if !actor.selected.iter().any(|selected| selected == id) {
        actor.selected.push(id.into());
    }
    let (audio, received) = mpsc::channel(MEDIA_QUEUE_BLOCKS);
    let (volume, _) = watch::channel(None);
    let (stop, mut stopped) = watch::channel(false);
    let task = tokio::spawn(async move {
        while stopped.changed().await.is_ok() {
            if *stopped.borrow() {
                return;
            }
        }
    });
    let route = actor.routes.entry(id.into()).or_default();
    route.wanted = true;
    route.generation += 1;
    route.phase = Phase::Streaming;
    route.session = Some(ActiveTarget {
        audio,
        volume,
        stop,
        task,
        progress: Arc::new(Mutex::new(TargetProgress::default())),
        started: Instant::now(),
    });
    actor.running = true;
    actor.playback.get_or_insert_with(|| {
        let now = Instant::now();
        PlaybackTimeline {
            started: now,
            schedule: MediaSchedule::new(now),
        }
    });
    received
}

fn target_identity(actor: &Actor, id: &str) -> (tokio::task::Id, u64, Instant) {
    let route = &actor.routes[id];
    let session = route.session.as_ref().expect("receiver session retained");
    assert!(route.wanted);
    assert!(route.cleanup.is_none());
    assert_eq!(route.phase, Phase::Streaming);
    assert!(!session.task.is_finished());
    (session.task.id(), route.generation, session.started)
}

fn pending_capture(actor: &mut Actor) -> tokio::task::Id {
    assert!(actor.capture_work.is_none());
    let task = tokio::spawn(std::future::pending::<
        Result<Option<CaptureSession>, CaptureTransitionError>,
    >());
    let id = task.id();
    actor.capture_work = Some(CaptureWork {
        generation: actor.capture_generation,
        task,
    });
    id
}

async fn shutdown(actor: &mut Actor) {
    actor.stop_all();
    if let Some(work) = actor.capture_work.take() {
        work.task.abort();
        let _ = work.task.await;
    }
    if let Some(task) = actor.discovery.take() {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = actor.source_watch_start.take() {
        task.abort();
        let _ = task.await;
    }
    if let Some(task) = actor.source_scan.take() {
        task.abort();
        let _ = task.await;
    }
    for route in actor.routes.values_mut() {
        if let Some(task) = route.cleanup.take() {
            task.await.unwrap();
        }
    }
}

#[tokio::test]
async fn source_selection_and_scope_change_retain_each_live_receiver() {
    let mut actor = actor();
    let _left = attach(&mut actor, "left");
    let _right = attach(&mut actor, "right");
    let _solo = attach(&mut actor, "solo");
    let identities = ["left", "right", "solo"].map(|id| (id, target_identity(&actor, id)));
    let playback_started = actor.playback.as_ref().unwrap().started;
    let capture_task = pending_capture(&mut actor);

    actor.command(Command::SelectSource {
        source_id: "speakers".into(),
    });
    assert_eq!(
        actor.capture_request.as_ref().unwrap().source_id,
        "speakers"
    );
    for (id, expected) in &identities {
        assert_eq!(target_identity(&actor, id), *expected);
    }

    actor.command(Command::SelectSourceMode {
        mode: SourceMode::Application,
    });
    assert!(actor.capture_request.is_none());
    assert!(lock(&actor.state).selected_source_id.is_empty());
    assert!(!lock(&actor.state).mute_local);
    actor.sources_discovered(
        SourceMode::Application,
        Ok(vec![CaptureSource {
            id: "process:42:100".into(),
            name: "Fixture player".into(),
            is_default: false,
        }]),
    );
    actor.command(Command::SelectSource {
        source_id: "process:42:100".into(),
    });
    assert_eq!(
        actor.capture_request.as_ref().unwrap().source_id,
        "process:42:100"
    );
    for (id, expected) in &identities {
        assert_eq!(target_identity(&actor, id), *expected);
    }
    assert_eq!(actor.capture_work.as_ref().unwrap().task.id(), capture_task);
    assert_eq!(actor.playback.as_ref().unwrap().started, playback_started);
    assert!(actor.running);
    assert!(actor.queue.is_empty());
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn manual_refresh_and_partial_discovery_leave_playing_sessions_intact() {
    let mut actor = actor();
    let _left = attach(&mut actor, "left");
    let _right = attach(&mut actor, "right");
    let _solo = attach(&mut actor, "solo");
    let identities = ["left", "right", "solo"].map(|id| (id, target_identity(&actor, id)));
    let discovery = actor.discovery.as_ref().unwrap().id();
    let capture_generation = actor.capture_generation;
    for devices in [
        Vec::new(),
        vec![receiver("left", true), receiver("new", false)],
    ] {
        actor.command(Command::Refresh {});
        actor.discovered(DiscoveryResult {
            devices: Ok(devices),
        });
        for (id, expected) in &identities {
            assert_eq!(target_identity(&actor, id), *expected);
        }
        assert_eq!(actor.discovery.as_ref().unwrap().id(), discovery);
        assert_eq!(actor.capture_generation, capture_generation);
        assert!(actor.running);
        assert!(actor.queue.is_empty());
    }
    assert_eq!(actor.selected, ["left", "right", "solo"]);
    assert!(!actor.online.contains("right"));
    assert!(actor.online.contains("new"));
    assert!(!actor.routes.contains_key("new"));
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn missing_capture_keeps_silent_media_clock_and_test_tone_running() {
    let mut actor = actor();
    let mut received = attach(&mut actor, "solo");
    let target = target_identity(&actor, "solo");
    let started = actor.playback.as_ref().unwrap().started;
    pending_capture(&mut actor);
    let mut expected_sequence = 0;

    for source in ["speakers", "headphones"] {
        actor.command(Command::SelectSource {
            source_id: source.into(),
        });
        let timeline = actor.playback.as_mut().unwrap();
        timeline.schedule.next_due = Instant::now() + Duration::from_secs(60);
        actor.audio();
        assert!(matches!(
            received.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let due = Instant::now() - Duration::from_millis(1);
        actor.playback.as_mut().unwrap().schedule.next_due = due;
        actor.audio();
        let mut blocks = 0;
        while let Ok(block) = received.try_recv() {
            assert_eq!(block.sequence, expected_sequence);
            assert_eq!(block.samples.len(), 882); // 10 ms of 44.1 kHz stereo.
            assert!(block.samples.iter().all(|sample| *sample == 0));
            expected_sequence += 1;
            blocks += 1;
        }
        assert!(blocks > 0);
        assert_eq!(
            actor.playback.as_ref().unwrap().schedule.next_due,
            due + BLOCK_PERIOD * blocks
        );
        assert_eq!(actor.playback.as_ref().unwrap().started, started);
    }

    actor.command(Command::TestTone {
        enabled: Some(true),
    });
    actor.playback.as_mut().unwrap().schedule.next_due = Instant::now();
    actor.audio();
    let tone = received.try_recv().unwrap();
    assert_eq!(tone.sequence, expected_sequence);
    assert!(tone.samples.iter().any(|sample| *sample != 0));
    assert!(tone
        .samples
        .chunks_exact(2)
        .all(|frame| frame[0] == frame[1]));
    assert!(actor.capture.is_none());
    assert!(actor.test_tone.as_ref().unwrap().position >= BLOCK_FRAMES);
    assert_eq!(target_identity(&actor, "solo"), target);
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn stale_capture_failures_do_not_replace_the_latest_request_or_error() {
    let mut actor = actor();
    let _audio = attach(&mut actor, "solo");
    let target = target_identity(&actor, "solo");
    pending_capture(&mut actor);
    for source in ["speakers", "headphones", "default"] {
        actor.command(Command::SelectSource {
            source_id: source.into(),
        });
    }
    let previous = actor.capture_work.take().unwrap();
    let old_generation = previous.generation;
    previous.task.abort();
    let cancelled = previous.task.await.err().unwrap();
    let current_generation = actor.capture_generation;
    assert_ne!(old_generation, current_generation);
    let current_task = pending_capture(&mut actor);
    actor.error("current_capture", "Current source status");
    actor.update(|s| s.capture.message = "Latest source is pending".into());

    for result in [
        Ok(Err(CaptureTransitionError::Start(
            airloom_capture::CaptureError::WorkerStopped,
        ))),
        Err(cancelled),
    ] {
        actor.finish_capture_work(old_generation, result);
        assert_eq!(actor.capture_request.as_ref().unwrap().source_id, "default");
        assert_eq!(actor.capture_generation, current_generation);
        assert_eq!(actor.capture_work.as_ref().unwrap().task.id(), current_task);
        assert_eq!(actor.global_error.as_ref().unwrap().code, "current_capture");
        assert_eq!(
            lock(&actor.state).capture.message,
            "Latest source is pending"
        );
        assert_eq!(target_identity(&actor, "solo"), target);
    }
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn a_new_receiver_registers_capture_while_a_previous_stop_is_pending() {
    let mut actor = actor();
    let _old_audio = attach(&mut actor, "solo");
    actor.capture_request = Some(actor.capture_config().unwrap());
    let old_capture_task = pending_capture(&mut actor);
    let old_generation = actor.capture_generation;
    actor.command(Command::Disconnect {
        device_id: None,
        request_id: None,
    });
    assert!(actor.capture_request.is_none());
    assert_eq!(actor.active_count(), 0);

    actor.command(Command::SelectSource {
        source_id: "headphones".into(),
    });
    actor.devices.push(receiver("next", false));
    actor.online.insert("next".into());
    let _new_audio = attach(&mut actor, "next");
    // The old receiver's network cleanup and the old capture transition are
    // independent; complete only the former before checking the new session.
    actor
        .routes
        .get_mut("solo")
        .unwrap()
        .cleanup
        .take()
        .unwrap()
        .await
        .unwrap();
    let new_target = target_identity(&actor, "next");
    actor.start_capture().unwrap();
    assert_eq!(
        actor.capture_request.as_ref().unwrap().source_id,
        "headphones"
    );
    assert_ne!(actor.capture_generation, old_generation);
    assert_eq!(
        actor.capture_work.as_ref().unwrap().task.id(),
        old_capture_task
    );
    assert_eq!(target_identity(&actor, "next"), new_target);
    // A capture failure can leave the same cleanup in flight. The UI's Retry
    // command must register a new input request without replacing this receiver.
    actor.stop_capture();
    assert!(actor.capture_request.is_none());
    actor.command(Command::Connect {
        device_id: Some("next".into()),
    });
    assert_eq!(
        actor.capture_request.as_ref().unwrap().source_id,
        "headphones"
    );
    assert_eq!(
        actor.capture_work.as_ref().unwrap().task.id(),
        old_capture_task
    );
    assert_eq!(target_identity(&actor, "next"), new_target);
    assert!(actor.queue.is_empty());
    shutdown(&mut actor).await;
}
