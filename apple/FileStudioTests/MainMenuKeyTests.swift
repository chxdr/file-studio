import AppKit
import XCTest
@testable import FileStudio

/// Key-cap text → menu key equivalents (`MainMenu.keyEquivalent`).
@MainActor
final class MainMenuKeyTests: XCTestCase {
    func testCommandLetter() throws {
        let (key, mods) = try XCTUnwrap(MainMenu.keyEquivalent("⌘N"))
        XCTAssertEqual(key, "n")
        XCTAssertEqual(mods, [.command])
    }

    func testModifiersCombine() throws {
        let (key, mods) = try XCTUnwrap(MainMenu.keyEquivalent("⇧⌘N"))
        XCTAssertEqual(key, "n")
        XCTAssertEqual(mods, [.command, .shift])
        let (_, all) = try XCTUnwrap(MainMenu.keyEquivalent("⌃⌥⇧⌘K"))
        XCTAssertEqual(all, [.control, .option, .shift, .command])
    }

    func testSpecialKeys() throws {
        XCTAssertEqual(try XCTUnwrap(MainMenu.keyEquivalent("⌘⌫")).0, "\u{8}")
        XCTAssertEqual(try XCTUnwrap(MainMenu.keyEquivalent("⌘↩")).0, "\r")
        XCTAssertEqual(try XCTUnwrap(MainMenu.keyEquivalent("Space")).0, " ")
        let down = try XCTUnwrap(MainMenu.keyEquivalent("⌘↓")).0
        XCTAssertEqual(down.unicodeScalars.first?.value, UInt32(NSDownArrowFunctionKey))
    }

    func testFunctionKeys() throws {
        let (f5, _) = try XCTUnwrap(MainMenu.keyEquivalent("F5"))
        XCTAssertTrue(MainMenu.isFunctionKey(f5))
        XCTAssertEqual(f5.unicodeScalars.first?.value, UInt32(NSF5FunctionKey))
        XCTAssertNil(MainMenu.keyEquivalent("F21"), "only F1–F20 exist")
        XCTAssertFalse(MainMenu.isFunctionKey("a"))
    }

    func testSequencesAndJunkAreNotMenuKeys() {
        XCTAssertNil(MainMenu.keyEquivalent("g g"), "key sequences can't be menu keys")
        XCTAssertNil(MainMenu.keyEquivalent("⌘Nope"))
    }
}
