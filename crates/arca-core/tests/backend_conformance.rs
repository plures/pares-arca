//! Backend conformance tests — both filesystem and sled must pass identical tests.

#[cfg(test)]
mod tests {
    use arca_core::backend::CacheBackend;
    use arca_core::sled_store::SledStore;
    use arca_core::store::CacheStore;

    fn test_backend_roundtrip(backend: &dyn CacheBackend) {
        assert!(!backend.has("test123"));
        assert_eq!(backend.count(), 0);

        let narinfo = "StorePath: /nix/store/test123-hello\nURL: nar/test123.nar.xz\n";
        backend.put_narinfo("test123", narinfo).unwrap();

        assert!(backend.has("test123"));
        assert_eq!(backend.get_narinfo("test123").unwrap(), narinfo);
        assert_eq!(backend.count(), 1);
        assert_eq!(backend.list_hashes(), vec!["test123"]);
        assert_eq!(backend.total_narinfo_size(), narinfo.len() as u64);
    }

    #[test]
    fn test_filesystem_backend_conformance() {
        let tmp = tempfile::tempdir().unwrap();
        let store = CacheStore::new(tmp.path().join("cache")).unwrap();
        test_backend_roundtrip(&store);
    }

    #[test]
    fn test_sled_backend_conformance() {
        let tmp = tempfile::tempdir().unwrap();
        let store = SledStore::new(tmp.path().join("db")).unwrap();
        test_backend_roundtrip(&store);
    }
}
