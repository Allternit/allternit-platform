import XCTest
@testable import AllternitFilesKit

/// The two pieces of the macOS File Provider (plan step 23) with logic of
/// their own:
///
/// - Finder badges: a conflict copy is recognised only by the exact name the
///   runtime gives it (SPEC section 4), never a file a user named
///   "(conflict ...)" by hand without the timestamp;
/// - mesh candidates go through the Desktop app's loopback proxy for that
///   exact host and port, and are left alone when the app runs none.
final class MacProviderTests: XCTestCase {
    func testConflictCopyNamesFollowTheRuntimeFormat() {
        XCTAssertTrue(FilesBadge.isConflictCopy("Lease (conflict iPhone 2026-11-15 1042).pdf"))
        XCTAssertTrue(FilesBadge.isConflictCopy("Lease (conflict Joe's MacBook Pro 2026-11-15 1042 2).pdf"))
        XCTAssertTrue(FilesBadge.isConflictCopy("notes (conflict Mac 2026-01-02 0905)"))
        XCTAssertTrue(FilesBadge.isConflictCopy("archive.tar (conflict Mac 2026-01-02 0905).gz"))

        XCTAssertFalse(FilesBadge.isConflictCopy("Lease.pdf"))
        XCTAssertFalse(FilesBadge.isConflictCopy("Lease (conflict).pdf"))
        XCTAssertFalse(FilesBadge.isConflictCopy("Lease (conflict iPhone 2026-11-15).pdf"))
        XCTAssertFalse(FilesBadge.isConflictCopy("Lease (conflict iPhone 2026-11-15 1042).pdf.bak (2)"))
    }

    func testBadgesComeMostImportantFirst() throws {
        let root = try decode(Root.self, """
        {"id":"r_shared","kind":"shared","name":"Shared","root_item_id":"i_root","cursor":"c1","provider_ready":true,
         "policy":{"read":true,"write":true,"shareable":true,"agent_process":false,"protected":true,"offline":false,
                   "replica":false,"versions":{"keep_count":5,"keep_days":30},"trash_days":30}}
        """)
        let item = try decode(Item.self, """
        {"id":"i_1","root_id":"r_shared","parent_id":"i_root","name":"Plan (conflict Mac 2026-11-15 0930).md","kind":"file",
         "size":3,"rev":"r1","meta_version":1,"created_at":"2026-11-15T15:30:00Z","modified_at":"2026-11-15T15:30:00Z",
         "modified_by":{"kind":"agent","id":"gizzi"},
         "permissions":{"list":true,"read":true,"write":true,"delete":true,"share":true,"comment":true,
                         "agent_process":false,"transfer":true,"execute":false}}
        """)
        XCTAssertEqual(FilesBadge.badges(for: item, root: root, pinned: true), [.conflict, .protected, .agent, .pinned, .shared])
        XCTAssertEqual(FilesBadge.badges(for: nil, root: nil, pinned: false), [])
    }

    func testMeshCandidateUsesTheAppsLoopbackProxyForThatHostOnly() throws {
        let group = "test.allternit.files.\(UUID().uuidString)"
        let store = FilesSharedStore(appGroup: group, keychainGroup: nil)
        defer { UserDefaults().removePersistentDomain(forName: group) }
        store.meshProxies = ["100.64.0.7:8013": "127.0.0.1:53122"]

        let proxied = store.meshProxyURL(for: try XCTUnwrap(URL(string: "http://100.64.0.7:8013/api/v1/files/v2")))
        XCTAssertEqual(proxied?.absoluteString, "http://127.0.0.1:53122/api/v1/files/v2")
        XCTAssertNil(store.meshProxyURL(for: try XCTUnwrap(URL(string: "http://100.64.0.8:8013/api/v1/files/v2"))))
        XCTAssertNil(store.meshProxyURL(for: try XCTUnwrap(URL(string: "http://100.64.0.7:9000/api/v1/files/v2"))))
    }

    private func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
        try JSONDecoder().decode(T.self, from: Data(json.utf8))
    }
}
