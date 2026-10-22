//! Actor selection and acknowledgement contracts. The bounded workers below
//! model the sender boundary; real RTSP volume reads/writes live in airplay tests.
use super::*;

fn actor() -> Actor {
    Actor::new(
        Arc::new(Mutex::new(Snapshot::default())),
        Arc::new(AtomicBool::new(false)),
    )
}

fn attach(
    actor: &mut Actor,
    id: &str,
    selected: bool,
    actual_volume: Option<f32>,
) -> watch::Receiver<Option<f32>> {
    let mut device = Device::manual("127.0.0.1".parse().unwrap(), 7000);
    device.id = id.into();
    actor.devices.push(device);
    if selected {
        actor.selected.push(id.into());
    }
    let (audio, _input) = mpsc::channel(1);
    let (volume, changes) = watch::channel(None);
    let (stop, mut stopped) = watch::channel(false);
    let task = tokio::spawn(async move {
        while stopped.changed().await.is_ok() {
            if *stopped.borrow() {
                break;
            }
        }
    });
    actor.routes.insert(
        id.into(),
        Route {
            wanted: true,
            phase: Phase::Streaming,
            session: Some(ActiveTarget {
                audio,
                volume,
                stop,
                task,
                progress: Arc::new(Mutex::new(TargetProgress {
                    metrics: SenderMetrics::default(),
                    volume: actual_volume,
                })),
                started: Instant::now(),
            }),
            ..Route::default()
        },
    );
    changes
}

fn observed(actor: &Actor, id: &str, volume: Option<f32>) {
    lock(&actor.routes[id].session.as_ref().unwrap().progress).volume = volume;
}

async fn shutdown(actor: &mut Actor) {
    actor.stop_all();
    actor.finish_capture_shutdown().await;
    for route in actor.routes.values_mut() {
        if let Some(task) = route.cleanup.take() {
            task.await.unwrap();
        }
    }
}

#[tokio::test]
async fn selected_receiver_readings_drive_known_mixed_and_unknown_without_writes() {
    let mut actor = actor();
    let one = attach(&mut actor, "one", true, Some(0.56));
    let two = attach(&mut actor, "two", true, Some(0.56));
    let other = attach(&mut actor, "other", false, Some(0.91));
    actor.synchronize();
    let state = lock(&actor.state).clone();
    assert_eq!(state.volume, 0.56);
    assert!(state.volume_known);
    assert!(!state.volume_mixed);

    observed(&actor, "one", Some(0.3));
    observed(&actor, "two", Some(0.7));
    actor.synchronize();
    let state = lock(&actor.state).clone();
    assert_eq!(state.volume, 0.3);
    assert!(state.volume_known && state.volume_mixed);
    assert!(state.diagnostics.unwrap().volume_mixed);

    observed(&actor, "two", None);
    actor.synchronize();
    assert!(!lock(&actor.state).volume_known);
    assert!(!lock(&actor.state).volume_mixed);
    observed(&actor, "one", Some(0.0));
    observed(&actor, "two", Some(0.0));
    actor.synchronize();
    assert_eq!(lock(&actor.state).volume, 0.0);
    assert!(lock(&actor.state).volume_known);
    assert!(!lock(&actor.state).volume_mixed);
    for watcher in [one, two, other] {
        assert_eq!(*watcher.borrow(), None);
        assert!(!watcher.has_changed().unwrap());
    }
    shutdown(&mut actor).await;
    actor.synchronize();
    assert!(!lock(&actor.state).volume_known);
}

#[tokio::test]
async fn user_adjustment_only_targets_selection_and_waits_for_actual_acknowledgement() {
    let mut actor = actor();
    let mut one = attach(&mut actor, "one", true, Some(0.3));
    let mut two = attach(&mut actor, "two", true, None);
    let other = attach(&mut actor, "other", false, Some(0.91));
    actor.command(Command::SetVolume { volume: 0.62 });
    one.changed().await.unwrap();
    two.changed().await.unwrap();
    assert_eq!(*one.borrow(), Some(0.62));
    assert_eq!(*two.borrow(), Some(0.62));
    assert!(!other.has_changed().unwrap());
    actor.synchronize();
    assert!(!lock(&actor.state).volume_known);
    assert_eq!(lock(&actor.state).volume, 0.3);
    observed(&actor, "one", Some(0.62));
    actor.synchronize();
    assert!(!lock(&actor.state).volume_known);
    observed(&actor, "two", Some(0.62));
    actor.synchronize();
    assert!(lock(&actor.state).volume_known);
    assert!(!lock(&actor.state).volume_mixed);
    assert_eq!(lock(&actor.state).volume, 0.62);
    assert_eq!(actor.routes["other"].volume(), Some(0.91));
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn adjustments_during_connect_are_scoped_and_never_become_a_startup_preference() {
    let mut actor = actor();
    actor.selected = vec!["connecting".into(), "idle".into()];
    actor.routes.insert(
        "connecting".into(),
        Route {
            wanted: true,
            ..Route::default()
        },
    );
    actor.routes.insert("idle".into(), Route::default());
    actor.command(Command::SetVolume { volume: 0.42 });
    assert_eq!(actor.routes["connecting"].requested_volume, Some(0.42));
    assert_eq!(actor.routes["idle"].requested_volume, None);
    actor.routes.insert(
        "later".into(),
        Route {
            wanted: true,
            ..Route::default()
        },
    );
    actor.selected.push("later".into());
    assert_eq!(actor.routes["later"].requested_volume, None);
    actor.stop_route("connecting");
    assert_eq!(actor.routes["connecting"].requested_volume, None);
    actor.command(Command::SetVolume { volume: 0.83 });
    assert_eq!(actor.routes["connecting"].requested_volume, None);
    assert_eq!(actor.routes["idle"].requested_volume, None);
    assert_eq!(actor.routes["later"].requested_volume, Some(0.83));
    shutdown(&mut actor).await;
}
