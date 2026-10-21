//! Core routing contracts. Workers below model only the bounded queue boundary;
//! they make no claim about AirPlay protocol acceptance or physical HomePods.
use super::*;

const PAIR_A: &str = "EAFA36AA-9785-54B2-A537-D9EE2A55CF1C";
const PAIR_B: &str = "9270BA34-547F-5E97-AB55-B39F6BC9579A";

fn group_id(pair: &str) -> String {
    format!("stereo:{}", pair.to_ascii_lowercase())
}

fn receiver(id: &str, pair: Option<&str>) -> Device {
    let mut device = Device::manual("192.0.2.10".parse().unwrap(), 7000);
    device.id = id.into();
    device.name = format!("Fixture {id}");
    device.model = Some("AudioAccessory5,1".into());
    if let Some(pair) = pair {
        device.txt.insert("tsid".into(), pair.into());
        device.txt.insert("gpn".into(), "Living room".into());
    }
    device
}

fn actor_with(devices: Vec<Device>) -> Actor {
    let snapshot = Snapshot {
        sources: vec![SourceView {
            id: "default".into(),
            name: "Fixture output".into(),
            kind: "endpoint",
            is_default: true,
        }],
        ..Snapshot::default()
    };
    let mut actor = Actor::new(
        Arc::new(Mutex::new(snapshot)),
        Arc::new(AtomicBool::new(false)),
    );
    actor.online = devices.iter().map(|device| device.id.clone()).collect();
    actor.devices = devices;
    actor
}

fn pair_and_solo() -> Actor {
    actor_with(vec![
        receiver("a", Some(PAIR_A)),
        receiver("b", Some(PAIR_A)),
        receiver("solo", None),
    ])
}

fn ids(values: impl IntoIterator<Item = impl AsRef<str>>) -> BTreeSet<String> {
    values
        .into_iter()
        .map(|value| value.as_ref().into())
        .collect()
}

fn attach(actor: &mut Actor, id: &str) -> mpsc::Receiver<MediaBlock> {
    actor.selected.push(id.into());
    let (audio, received) = mpsc::channel(4);
    let (volume, _) = watch::channel(0.5);
    let (stop, mut stopped) = watch::channel(false);
    let task = tokio::spawn(async move {
        while stopped.changed().await.is_ok() {
            if *stopped.borrow() {
                break;
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
        progress: Arc::new(Mutex::new(SenderMetrics::default())),
        started: Instant::now(),
    });
    actor.running = true;
    received
}

async fn shutdown(actor: &mut Actor) {
    actor.stop_all();
    for route in actor.routes.values_mut() {
        if let Some(task) = route.cleanup.take() {
            task.await.unwrap();
        }
    }
}

#[test]
fn one_logical_pair_requires_valid_tsid_and_does_not_infer_channel_roles() {
    let mut actor = pair_and_solo();
    actor.devices[0].name = "User named right".into();
    actor.devices[1].name = "User named left".into();
    actor.devices[0].txt.insert("tsm".into(), "1".into());
    actor.devices[1].txt.insert("tsm".into(), "0".into());
    // Same public name and transient playback-group ID are insufficient.
    actor.devices[2].name = "Living room".into();
    actor.devices[2]
        .txt
        .insert("gpn".into(), "Living room".into());
    actor.devices[2].txt.insert("gid".into(), PAIR_A.into());
    actor.devices[2]
        .txt
        .insert("tsid".into(), "not-a-uuid".into());
    actor.synchronize();
    let snapshot = lock(&actor.state).clone();
    assert_eq!(snapshot.devices.len(), 2);
    let group = snapshot
        .devices
        .iter()
        .find(|device| device.kind == "native_stereo")
        .unwrap();
    assert_eq!(group.id, group_id(PAIR_A));
    assert_eq!(group.expected_members, 2);
    assert!(group.group_complete);
    assert_eq!(
        ids(group.members.iter().map(|member| &member.id)),
        ids(["a", "b"])
    );
    assert_eq!(
        group
            .members
            .iter()
            .find(|member| member.id == "a")
            .unwrap()
            .name,
        "User named right"
    );
    for member in serde_json::to_value(&group.members)
        .unwrap()
        .as_array()
        .unwrap()
    {
        assert!(member.get("channel").is_none());
        assert!(member.get("left").is_none());
        assert!(member.get("right").is_none());
    }
    assert_eq!(
        snapshot
            .devices
            .iter()
            .find(|device| device.id == "solo")
            .unwrap()
            .kind,
        "device"
    );
}

#[tokio::test]
async fn selecting_connecting_and_cancelling_a_group_never_adopts_a_third_receiver() {
    let mut actor = pair_and_solo();
    let group = group_id(PAIR_A);
    actor.command(Command::ToggleDevice {
        device_id: group.clone(),
        selected: Some(true),
    });
    assert_eq!(ids(&actor.selected), ids(["a", "b"]));
    actor.synchronize();
    assert_eq!(lock(&actor.state).selected_device_ids, vec![group.clone()]);
    actor.command(Command::Connect {
        device_id: Some(group.clone()),
    });
    actor.command(Command::Connect {
        device_id: Some(group.clone()),
    });
    assert_eq!(
        ids(actor.queue.iter().map(|request| &request.device_id)),
        ids(["a", "b"])
    );
    assert_eq!(actor.queue.len(), 2);
    assert!(!actor.routes.contains_key("solo"));
    actor.command(Command::ToggleDevice {
        device_id: group,
        selected: Some(false),
    });
    assert!(actor.selected.is_empty());
    assert!(actor.queue.is_empty());
    assert!(!actor.running);
    assert!(actor.routes.values().all(|route| !route.wanted));
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn incomplete_or_conflicting_native_group_does_not_start_a_member() {
    for devices in [
        vec![receiver("a", Some(PAIR_A)), receiver("solo", None)],
        vec![
            receiver("a", Some(PAIR_A)),
            receiver("b", Some(PAIR_A)),
            receiver("c", Some(PAIR_A)),
        ],
    ] {
        let mut actor = actor_with(devices);
        actor.command(Command::Connect {
            device_id: Some(group_id(PAIR_A)),
        });
        assert!(actor.queue.is_empty());
        assert!(actor.work.is_none());
        assert_eq!(actor.active_count(), 0);
        assert!(actor
            .routes
            .values()
            .all(|route| route.phase == Phase::Error && !route.wanted));
        actor.synchronize();
        let state = lock(&actor.state).clone();
        let group = state
            .devices
            .iter()
            .find(|device| device.kind == "native_stereo")
            .unwrap();
        assert!(!group.group_complete);
        assert_eq!(group.error.as_ref().unwrap().code, "stereo_incomplete");
        shutdown(&mut actor).await;
    }
}

#[tokio::test]
async fn group_members_receive_the_same_first_and_following_complete_stereo_blocks() {
    let mut actor = pair_and_solo();
    let mut a = attach(&mut actor, "a");
    let mut b = attach(&mut actor, "b");
    for frame in 0..3 {
        let samples: Arc<[i16]> = (0..BLOCK_FRAMES)
            .flat_map(|sample| [sample as i16, -(frame + 1)])
            .collect::<Vec<_>>()
            .into();
        actor.fan_out(samples.clone());
        let a = a.try_recv().unwrap();
        let b = b.try_recv().unwrap();
        assert_eq!(a.sequence, b.sequence);
        assert_eq!(a.sequence, frame as u64);
        assert_eq!(a.captured_at, b.captured_at);
        assert!(Arc::ptr_eq(&a.samples, &samples));
        assert!(Arc::ptr_eq(&b.samples, &samples));
        assert_eq!(a.samples.len(), BLOCK_FRAMES * 2);
    }
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn failure_of_one_member_retries_its_whole_pair_and_preserves_other_outputs() {
    let mut actor = actor_with(vec![
        receiver("a", Some(PAIR_A)),
        receiver("b", Some(PAIR_A)),
        receiver("c", Some(PAIR_B)),
        receiver("d", Some(PAIR_B)),
        receiver("solo", None),
    ]);
    let _a = attach(&mut actor, "a");
    let _b = attach(&mut actor, "b");
    let mut c = attach(&mut actor, "c");
    let mut d = attach(&mut actor, "d");
    let mut solo = attach(&mut actor, "solo");
    let previous_b_generation = actor.routes["b"].generation;
    actor.target_failed(TargetFailure {
        identity: TargetIdentity {
            device_id: "a".into(),
            generation: actor.routes["a"].generation,
        },
        message: "fixture member transport failure".into(),
    });
    for member in ["a", "b"] {
        let route = &actor.routes[member];
        assert!(route.session.is_none());
        assert_eq!(route.phase, Phase::Reconnecting);
        assert_eq!(route.retries, 1);
        assert!(route.wanted);
    }
    assert_eq!(actor.routes["a"].retry_at, actor.routes["b"].retry_at);
    actor.target_failed(TargetFailure {
        identity: TargetIdentity {
            device_id: "b".into(),
            generation: previous_b_generation,
        },
        message: "late peer failure from the retired group".into(),
    });
    assert_eq!(actor.routes["a"].retries, 1);
    assert_eq!(actor.routes["b"].retries, 1);
    for member in ["c", "d", "solo"] {
        assert!(actor.routes[member].session.is_some());
        assert_eq!(actor.routes[member].retries, 0);
    }
    actor.fan_out(vec![17; BLOCK_FRAMES * 2].into());
    for received in [&mut c, &mut d, &mut solo] {
        assert_eq!(received.try_recv().unwrap().samples[0], 17);
    }
    actor.synchronize();
    assert_eq!(lock(&actor.state).state, Phase::Streaming);
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn disconnecting_one_member_stops_both_and_keeps_independent_audio() {
    let mut actor = pair_and_solo();
    let _a = attach(&mut actor, "a");
    let _b = attach(&mut actor, "b");
    let mut solo = attach(&mut actor, "solo");
    actor.command(Command::Disconnect {
        device_id: Some("a".into()),
        request_id: None,
    });
    assert!(actor.routes["a"].session.is_none());
    assert!(actor.routes["b"].session.is_none());
    assert!(!actor.routes["a"].wanted && !actor.routes["b"].wanted);
    assert!(actor.routes["solo"].session.is_some());
    assert_eq!(ids(&actor.selected), ids(["a", "b", "solo"]));
    actor.fan_out(vec![9; BLOCK_FRAMES * 2].into());
    assert_eq!(solo.try_recv().unwrap().samples[0], 9);
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn discovery_changes_preserve_selection_and_the_existing_auth_request() {
    let mut actor = pair_and_solo();
    actor.command(Command::Connect {
        device_id: Some(group_id(PAIR_A)),
    });
    actor.prompt("a", AuthenticationKind::Password, false);
    actor.routes.get_mut("a").unwrap().access.password =
        Some(Zeroizing::new("fixture password".into()));
    let request = actor.auth.as_ref().unwrap().request_id;
    let generation = actor.routes["a"].generation;
    let mut updated = actor.devices.clone();
    updated[0].txt.insert("gid".into(), PAIR_B.into());
    updated[0].txt.insert("igl".into(), "0".into());
    updated[1].txt.insert("pgid".into(), PAIR_B.into());
    updated.push(receiver("new", None));
    actor.discovered(DiscoveryResult {
        devices: Ok(updated),
    });
    assert_eq!(ids(&actor.selected), ids(["a", "b"]));
    assert!(actor.matches_auth(request));
    assert_eq!(actor.routes["a"].generation, generation);
    assert_eq!(
        actor.routes["a"]
            .access
            .password
            .as_ref()
            .map(|value| value.as_str()),
        Some("fixture password")
    );
    assert!(!actor.routes.contains_key("new"));
    assert_eq!(
        actor.target_members(&group_id(PAIR_A)).unwrap(),
        vec!["a", "b"]
    );
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn absent_member_is_retained_offline_without_inventing_a_new_group() {
    let mut actor = pair_and_solo();
    actor.select(&group_id(PAIR_A), true);
    actor.discovered(DiscoveryResult {
        devices: Ok(vec![receiver("a", Some(PAIR_A)), receiver("solo", None)]),
    });
    assert_eq!(ids(&actor.selected), ids(["a", "b"]));
    assert!(actor.known_device("b").is_some());
    assert!(!actor.online.contains("b"));
    actor.synchronize();
    let state = lock(&actor.state).clone();
    let group = state
        .devices
        .iter()
        .find(|device| device.id == group_id(PAIR_A))
        .unwrap();
    assert_eq!(group.members.len(), 2);
    assert!(!group.group_complete);
    actor.command(Command::Connect {
        device_id: Some(group_id(PAIR_A)),
    });
    assert!(actor.queue.is_empty());
    shutdown(&mut actor).await;
}

#[tokio::test]
async fn explicit_stereo_identity_change_retires_both_old_sessions_before_regrouping() {
    for replacement in [None, Some(PAIR_B)] {
        let mut actor = pair_and_solo();
        let _a = attach(&mut actor, "a");
        let _b = attach(&mut actor, "b");
        let mut solo = attach(&mut actor, "solo");
        actor.discovered(DiscoveryResult {
            devices: Ok(vec![
                receiver("a", replacement),
                receiver("b", Some(PAIR_A)),
                receiver("solo", None),
            ]),
        });
        assert!(
            actor.routes["a"].session.is_none(),
            "updated member must leave its old live group"
        );
        assert!(
            actor.routes["b"].session.is_none(),
            "old partner must not continue with a retired group clock"
        );
        assert!(!actor.routes["a"].wanted && !actor.routes["b"].wanted);
        assert_eq!(ids(&actor.selected), ids(["a", "b", "solo"]));
        assert!(actor.routes["solo"].session.is_some());
        assert_eq!(actor.target_members(&group_id(PAIR_A)).unwrap(), vec!["b"]);
        actor.fan_out(vec![33; BLOCK_FRAMES * 2].into());
        assert_eq!(solo.try_recv().unwrap().samples[0], 33);
        shutdown(&mut actor).await;
    }
}

#[test]
fn native_stereo_test_tone_is_left_then_silence_then_right() {
    let rate = airloom_airplay::SAMPLE_RATE as usize;
    let mut tone = TestTone::new(true);
    // Nonzero source material must disappear during the test's steady portion.
    let mut samples = vec![0.4; TEST_TONE_FRAMES * 2];
    tone.apply(&mut samples);
    assert!(tone.finished());
    assert_eq!(tone.remaining_ms(), 0);
    let frames: Vec<_> = samples.chunks_exact(2).collect();
    // The first and last 20ms deliberately crossfade the original source.
    let left = &frames[rate / 10..rate * 14 / 10];
    let silence = &frames[rate * 14 / 10..rate * 16 / 10];
    let right = &frames[rate * 16 / 10..rate * 29 / 10];
    assert!(left.iter().all(|frame| frame[1] == 0.0));
    assert!(left.iter().any(|frame| frame[0].abs() > 0.1));
    assert!(silence
        .iter()
        .all(|frame| frame[0] == 0.0 && frame[1] == 0.0));
    assert!(right.iter().all(|frame| frame[0] == 0.0));
    assert!(right.iter().any(|frame| frame[1].abs() > 0.1));

    let mut ordinary = TestTone::new(false);
    let mut stereo = vec![0.0; rate / 5 * 2];
    ordinary.apply(&mut stereo);
    assert!(stereo.chunks_exact(2).all(|frame| frame[0] == frame[1]));
    assert!(stereo.iter().any(|sample| sample.abs() > 0.1));
}
