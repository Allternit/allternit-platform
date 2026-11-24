import XCTest
@testable import AllternitFilesKit

/// Regression for #2449: after the server reset a root's change cursor
/// (`reset_required`), the working set re-listed only that root's top
/// folder, so a file deleted in a nested folder while the cursor was stale
/// stayed in Files until the user opened that folder again.
///
/// Device tree for the "Allternit" root (it_home):
///   it_home/
///     Projects/ (it_proj)
///       plan.md (it_plan)
///       Old/ (it_old)
///         draft.md (it_draft)   <- deleted on the computer meanwhile
///     Inbox/ (it_inbox, its own root with its own cursor)
///       w9.pdf (it_w9)
final class FilesResyncTests: XCTestCase {
    private let materialized = [
        FilesMaterializedEntry(id: "it_proj", parentId: "it_home", isFolder: true),
        FilesMaterializedEntry(id: "it_plan", parentId: "it_proj", isFolder: false),
        FilesMaterializedEntry(id: "it_old", parentId: "it_proj", isFolder: true),
        FilesMaterializedEntry(id: "it_draft", parentId: "it_old", isFolder: false),
        FilesMaterializedEntry(id: "it_inbox", parentId: "it_home", isFolder: true),
        FilesMaterializedEntry(id: "it_w9", parentId: "it_inbox", isFolder: false),
    ]

    func testResetRelistsEveryMaterializedFolderBelowTheRootButNotOtherRoots() {
        let folders = FilesResync.containers(below: "it_home", materialized: materialized, otherRoots: ["it_inbox"])
        XCTAssertEqual(folders, ["it_home", "it_proj", "it_old"],
                       "nested folders must be listed again, parents first; Inbox has its own cursor")
    }

    func testANestedDeletionMissedWhileTheCursorWasStaleIsReported() {
        // The fresh listing of Old/ no longer has draft.md.
        let gone = FilesResync.gone(in: "it_old", fresh: [], materialized: materialized, roots: ["it_home", "it_inbox"])
        XCTAssertEqual(gone, ["it_draft"])
    }

    func testRootFoldersAreNeverReportedGoneByAListing() {
        // A listing of ~/Allternit that has no link entry for Inbox must not delete it.
        let gone = FilesResync.gone(in: "it_home", fresh: ["it_proj"], materialized: materialized, roots: ["it_home", "it_inbox"])
        XCTAssertEqual(gone, [])
    }
}
