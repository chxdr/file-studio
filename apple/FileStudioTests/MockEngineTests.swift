import XCTest
@testable import FileStudio

/// The mock engine is what the UI runs on without Rust, so its tab and
/// window bookkeeping has to behave like the real core's.
@MainActor
final class MockEngineTests: XCTestCase {
    func testRestoreSessionCreatesOneWindowWithOneTab() {
        let engine = MockEngine()
        let state = engine.restoreSession()
        XCTAssertEqual(state.windows.count, 1)
        XCTAssertEqual(state.windows[0].panes[0].tabs.count, 1)
        XCTAssertEqual(engine.activeTab()?.location(), "/Mock")
    }

    func testOpenAndCloseTabRoundTrip() throws {
        let engine = MockEngine()
        _ = engine.restoreSession()
        let tab = try engine.openTab(path: "/Mock/Documents")
        XCTAssertEqual(tab.location(), "/Mock/Documents")
        XCTAssertEqual(engine.workspaceState().windows[0].panes[0].tabs.count, 2)

        try engine.closeTab(tab.state().id)
        XCTAssertEqual(engine.workspaceState().windows[0].panes[0].tabs.count, 1)
    }

    func testNavigationKeepsBackHistory() throws {
        let engine = MockEngine()
        _ = engine.restoreSession()
        let tab = try XCTUnwrap(engine.activeTab())
        _ = try tab.go("/Mock/Documents")
        XCTAssertEqual(tab.location(), "/Mock/Documents")
        _ = try tab.back()
        XCTAssertEqual(tab.location(), "/Mock")
    }

    func testCloseWindowEmptiesWorkspace() throws {
        let engine = MockEngine()
        let id = try XCTUnwrap(engine.restoreSession().windows.first?.id)
        try engine.closeWindow(id)
        XCTAssertTrue(engine.workspaceState().windows.isEmpty)
    }
}
