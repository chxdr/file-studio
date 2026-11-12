// TourDebugSteps.swift — DEBUG-only script steps for the power tour
// (registered with DebugAutomation):
//
//   tour:<n>          opens the tour at card n (0-based), or moves there; demos freeze at
//                     their key frame so snapshots are stable (tour:<n>,<phase> picks the phase)
//   tour:menu         opens it as Help › File Studio Power Tour does (performs app.power_tour)
//   tour:close        closes it
//   tourkey:<command>,<keys>   rebinds a command's primary key in the core (empty keys: reset),
//                     to show the tour following the change
//   tourlog           TOUR lines: the card, its keys as read from the core, and the demo text

#if DEBUG
import AppKit

@MainActor
enum TourDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("tour") { arg, m in
            switch arg {
            case "close": TourController.close()
            case "menu": m.perform(Invocation(id: "app.power_tour"), via: .menu, window: m.keyWindowID)
            default:
                let parts = arg.split(separator: ",").map(String.init)
                let i = Int(parts.first ?? "") ?? 0
                TourController.present(model: m, card: i)
                guard let s = TourController.state else { return }
                s.frozenPhase = parts.count > 1 ? Double(parts[1]) : TourDemoStage.keyFrame(s.cards[min(max(i, 0), s.cards.count - 1)].demo)
            }
        }
        DebugAutomation.register("tourkey") { arg, m in
            let parts = arg.split(separator: ",", maxSplits: 1).map(String.init)
            do {
                if parts.count < 2 || parts[1].isEmpty { try m.engine.resetCommandKeys(parts[0]) } else {
                    try m.engine.setCommandKey(parts[0], primary: true, keys: parts[1])
                }
                print("TOURKEY \(parts[0]) now \(m.engine.commandKeys(parts[0]))")
            } catch { print("TOURKEY error \(error)") }
            // Redraw the open card with the new keys.
            if let s = TourController.state { s.frozenPhase = (s.frozenPhase ?? 0.5) + 0.0001 }
        }
        DebugAutomation.register("tourlog") { _, m in
            guard let s = TourController.state else { print("TOUR closed"); return }
            let c = s.card
            print("TOUR card=\(s.index + 1)/\(s.cards.count) id=\(c.id) title=\(c.title)")
            for k in c.keys {
                let caps = TourKeys.caps(k, keysFor: { m.engine.commandKeys($0) })
                let what: String = switch k {
                case .command(let id, let cap): "\(id) (\(cap))"
                case .typed(_, let cap): "typed (\(cap))"
                case .literal(_, let cap): "literal (\(cap))"
                }
                print("TOUR key \(what): \(caps.isEmpty ? "—" : caps.joined(separator: " or ")) | spoken: \(caps.map(TourKeys.spoken).joined(separator: ", or "))")
            }
            print("TOUR demo: \(c.demoDescription)")
        }
    }
}
#endif
