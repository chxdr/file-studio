import XCTest
@testable import FileStudio

/// The batch-rename sheet's model on the mock engine: it sends every edit
/// to the session and reads back the counts; the rename rules themselves are
/// tested in Rust (fstudio-rename, fstudio-ops, fstudio-core).
@MainActor
final class BatchRenameTests: XCTestCase {
    func testTheModelPushesStepsAndCountsRows() throws {
        let engine = MockEngine()
        let session = try engine.batchRename(tab: nil, paths: ["/Mock/a.txt", "/Mock/b.txt"])
        let model = BatchRenameModel(session: session, engine: engine, itemCount: 2)
        XCTAssertEqual(model.summary.total, 2)
        XCTAssertFalse(model.canApply, "nothing changes on the mock")
        let before = model.summary.version
        model.add(.replace)
        XCTAssertEqual(model.steps.count, 2)
        XCTAssertGreaterThan(model.summary.version, before)
        XCTAssertEqual(session.steps().count, 2)
        XCTAssertEqual(session.rows(start: 1, count: 5).map(\.oldName), ["b.txt"])
        model.remove(model.steps[0].id)
        model.remove(model.steps[0].id)
        XCTAssertEqual(model.steps.count, 1, "an empty list gets a template step back")
    }

    func testNewStepsHaveUsefulDefaults() {
        XCTAssertEqual(RenameStep(kind: .sequence).position, .end)
        XCTAssertEqual(RenameStep(kind: .insert).position, .start)
        XCTAssertEqual(RenameStep(kind: .template).template, "{name}")
        let a = RenameStep(kind: .trim)
        var b = RenameStep(kind: .trim)
        XCTAssertTrue(a.sameSettings(as: b))
        b.count = 4
        XCTAssertFalse(a.sameSettings(as: b))
    }

    func testTheCommandRequestHasAHandlerName() {
        XCTAssertEqual(ShellRequest.batchRename(tab: 1, paths: []).kind, "batchRename")
    }
}
