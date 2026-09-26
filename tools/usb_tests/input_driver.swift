// Test input driver for the Mac: posts keyboard and pointer events at the
// HID level, as the built-in keyboard and trackpad would, on request.
// Commands, one per line, written to /tmp/mac_input.cmd:
//   hotkey          Command+Esc
//   push-left N     move the pointer left N steps of 20 points (past the edge)
//   push-right N    the same to the right
//   where           log where the pointer is
// Never clicks and never types characters. Logs to /tmp/mac_input.log.
// Must run inside Terminal (its Accessibility permission lets it post).
import CoreGraphics
import Foundation

let commandPath = "/tmp/mac_input.cmd"
let logPath = "/tmp/mac_input.log"

func log(_ text: String) {
    let format = DateFormatter()
    format.dateFormat = "HH:mm:ss.SSS"
    let line = "\(format.string(from: Date())) \(text)\n"
    if let handle = FileHandle(forWritingAtPath: logPath) {
        handle.seekToEndOfFile()
        handle.write(line.data(using: .utf8)!)
        handle.closeFile()
    } else {
        FileManager.default.createFile(atPath: logPath, contents: line.data(using: .utf8))
    }
}

let source = CGEventSource(stateID: .hidSystemState)

func key(_ code: CGKeyCode, down: Bool, flags: CGEventFlags) {
    guard let event = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down) else {
        log("could not create a key event")
        return
    }
    event.flags = flags
    event.post(tap: .cghidEventTap)
}

func hotkey() {
    key(0x37, down: true, flags: .maskCommand)      // left Command
    usleep(60_000)
    key(0x35, down: true, flags: .maskCommand)      // Escape
    usleep(60_000)
    key(0x35, down: false, flags: .maskCommand)
    usleep(60_000)
    key(0x37, down: false, flags: [])
    usleep(60_000)
}

func pointer() -> CGPoint {
    return CGEvent(source: nil)?.location ?? .zero
}

func move(dx: Double, dy: Double) {
    let from = pointer()
    let to = CGPoint(x: from.x + dx, y: from.y + dy)
    guard let event = CGEvent(mouseEventSource: source, mouseType: .mouseMoved, mouseCursorPosition: to,
                              mouseButton: .left) else { return }
    event.setIntegerValueField(.mouseEventDeltaX, value: Int64(dx))
    event.setIntegerValueField(.mouseEventDeltaY, value: Int64(dy))
    event.post(tap: .cghidEventTap)
}

func push(dx: Double, steps: Int) {
    for _ in 0..<steps {
        move(dx: dx, dy: 0)
        usleep(10_000)
    }
}

log("started; allowed to post events: \(CGPreflightPostEventAccess())")
while true {
    if let text = try? String(contentsOfFile: commandPath, encoding: .utf8), !text.isEmpty {
        try? "".write(toFile: commandPath, atomically: false, encoding: .utf8)
        for line in text.split(separator: "\n") {
            let parts = line.split(separator: " ")
            guard let command = parts.first else { continue }
            let count = parts.count > 1 ? Int(parts[1]) ?? 60 : 60
            switch command {
            case "hotkey":
                hotkey()
                log("hotkey (Command+Esc)")
            case "push-left":
                let start = pointer()
                push(dx: -20, steps: count)
                log("pushed left \(count) steps from \(start) to \(pointer())")
            case "push-right":
                let start = pointer()
                push(dx: 20, steps: count)
                log("pushed right \(count) steps from \(start) to \(pointer())")
            case "where":
                log("pointer at \(pointer())")
            default:
                log("unknown command: \(line)")
            }
        }
    }
    usleep(100_000)
}
