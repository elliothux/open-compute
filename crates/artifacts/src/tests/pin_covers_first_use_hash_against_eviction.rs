use super::*;

#[tokio::test]
async fn pin_covers_first_use_hash_against_eviction() {
    let payload = b"first-use-hash-pin-body";
    let digest = hex::encode(Sha256::digest(payload));
    let artifact = ArtifactRef::new(ARTIFACT_KEY_VERSION, &digest, payload.len() as u64).unwrap();
    let tmp = TempDir::new().unwrap();
    let dest = cache_entry_path(tmp.path(), &digest);
    fs::create_dir_all(dest.parent().unwrap()).unwrap();
    fs::write(&dest, payload).unwrap();
    let cache = Arc::new(
        ArtifactCache::open(
            tmp.path().to_path_buf(),
            cache_config(payload.len() as u64),
            StartupId::generate(),
        )
        .unwrap(),
    );
    assert!(cache.is_indexed_for_test(&digest));

    struct Pause {
        in_hash: bool,
        release: bool,
    }
    let pause = Arc::new((
        Mutex::new(Pause {
            in_hash: false,
            release: false,
        }),
        Condvar::new(),
    ));
    let pause_hook = Arc::clone(&pause);
    let _guard = install_hash_pause(Arc::new(move || {
        let (lock, cv) = &*pause_hook;
        let mut g = lock.lock().unwrap();
        g.in_hash = true;
        cv.notify_all();
        while !g.release {
            g = cv.wait(g).unwrap();
        }
    }));

    let cache_hit = Arc::clone(&cache);
    let artifact_hit = artifact.clone();
    let join = std::thread::spawn(move || cache_hit.try_hit_for_test(&artifact_hit));

    {
        let (lock, cv) = &*pause;
        let mut g = lock.lock().unwrap();
        while !g.in_hash {
            g = cv.wait(g).unwrap();
        }
    }

    cache.evict_if_needed().await.unwrap();
    assert!(
        dest.exists(),
        "evictor must not unlink a hashing first-use pin"
    );
    assert!(cache.is_indexed_for_test(&digest));

    {
        let (lock, cv) = &*pause;
        let mut g = lock.lock().unwrap();
        g.release = true;
        cv.notify_all();
    }

    let mut pinned = join.join().unwrap().unwrap().unwrap();
    assert_eq!(pinned.read_all().unwrap(), payload);
    assert!(dest.exists());
    assert!(cache.is_indexed_for_test(&digest));
    drop(pinned);
}
