use super::support::*;
use crate::project::ProjectFile;
use crate::session::Session;

#[test]
fn projects_are_written_only_as_loom_files() {
    let root = fixtures_dir();
    let text = std::fs::read_to_string(root.join("demo.loom")).unwrap();
    let project = crate::ProjectFile::parse(&text).unwrap();
    let dir = TempDir::new("write");
    let mut session = Session::new();
    session.open_path(&root.join("cluster_drive.slog")).unwrap();
    let view = crate::ProjectView {
        view: project.view.clone(),
        bookmarks: project.bookmarks.clone(),
        notes: project.notes.clone(),
        cursor_a_us: project.cursor_a_us,
        cursor_b_us: project.cursor_b_us,
        cluster: project.cluster.clone(),
    };
    assert!(session
        .write_project(&dir.join("startup.bat"), &view)
        .is_err());
    assert!(!dir.join("startup.bat").exists());
    session
        .write_project(&dir.join("drive.LOOM"), &view)
        .unwrap();
    assert!(dir.join("drive.LOOM").is_file());
}

#[test]
fn projects_keep_cluster_assignments() {
    let root = fixtures_dir();
    let text = std::fs::read_to_string(root.join("demo.loom")).unwrap();
    let mut project = ProjectFile::parse(&text).unwrap();
    assert!(project.cluster.is_empty(), "older projects have none");
    project.cluster.insert("lamp1".into(), "led_cmd_19".into());
    project
        .cluster
        .insert("speed".into(), "SteeringAngle".into());
    project.cluster.insert("lamp2".into(), "   ".into());
    let saved = project.to_json().unwrap();
    let reopened = ProjectFile::parse(&saved).unwrap();
    assert_eq!(
        reopened.cluster.get("lamp1").map(String::as_str),
        Some("led_cmd_19")
    );
    assert_eq!(
        reopened.cluster.get("speed").map(String::as_str),
        Some("SteeringAngle")
    );
    assert!(
        !reopened.cluster.contains_key("lamp2"),
        "a blank assignment is dropped"
    );
    let opened = Session::new()
        .load_project_json(&saved, Some(&root))
        .unwrap();
    assert_eq!(opened.project.cluster.len(), 2);
}

#[test]
fn projects_do_not_follow_network_paths() {
    use crate::project::is_network_path;
    for network in [
        r"\\server\share\drive.slog",
        "//server/share/drive.slog",
        r"\\?\UNC\server\x",
    ] {
        assert!(is_network_path(network), "{network}");
    }
    for local in [
        r"C:\logs\drive.slog",
        "fixtures/drive.slog",
        "drive.slog",
        "/home/me/drive.slog",
    ] {
        assert!(!is_network_path(local), "{local}");
    }

    let root = fixtures_dir();
    let project = |log: &str, map: &str, compare: &str| {
        format!(
            r#"{{"format":"signal-loom","version":1,"logPath":{log:?},"signalMapPath":{map:?},
            "comparePath":{compare:?},"view":{{"playheadUs":0,"spanUs":1000000,"plotted":[]}}}}"#
        )
    };
    let err = Session::new()
        .load_project_json(&project(r"\\server\share\drive.slog", "", ""), Some(&root))
        .unwrap_err();
    assert!(err.to_string().contains("network path"), "{err}");

    let opened = Session::new()
        .load_project_json(
            &project(
                "cluster_drive.slog",
                r"\\server\maps\x.dbc",
                "//server/logs/b.slog",
            ),
            Some(&root),
        )
        .unwrap();
    assert!(
        opened.summary.map_label.is_none(),
        "the network map is not read"
    );
    let network_notes = opened
        .warnings
        .iter()
        .filter(|warning| warning.contains("network path"))
        .count();
    assert_eq!(network_notes, 2, "{:?}", opened.warnings);
}

#[test]
fn oversized_text_files_are_refused() {
    let dir = TempDir::new("cap");
    let file = dir.join("big.dbc");
    std::fs::write(&file, vec![b'x'; 2 * 1024 * 1024 + 1]).unwrap();
    let err = crate::project::read_text_capped(&file, 2 * 1024 * 1024, "signal map").unwrap_err();
    assert!(err.to_string().contains("at most 2 MB"), "{err}");
    assert!(crate::project::read_text_capped(&file, 3 * 1024 * 1024, "signal map").is_ok());
}

#[test]
fn exports_save_to_disk_only_as_csv_and_slog() {
    let session = open_cluster_drive();
    let dir = TempDir::new("export");
    let names = vec!["VehicleSpeed".to_string()];

    let csv = dir.join("speed.csv");
    let bytes = session
        .save_csv(&csv, &names, 0, 5_000_000)
        .unwrap()
        .text
        .len() as u64;
    let written = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(bytes, written.len() as u64);
    assert_eq!(written, session.export_csv(&names, 0, 5_000_000).unwrap());

    let slog = dir.join("trim.SLOG");
    session.save_slog(&slog, 0, 5_000_000).unwrap();
    assert!(std::fs::read_to_string(&slog)
        .unwrap()
        .starts_with("SLOGv1\n"));

    for refused in ["speed.bat", "speed", "trim.slog.exe"] {
        assert!(session
            .save_csv(&dir.join(refused), &names, 0, 5_000_000)
            .is_err());
        assert!(session.save_slog(&dir.join(refused), 0, 5_000_000).is_err());
        assert!(!dir.join(refused).exists(), "{refused} must not be created");
    }
}
