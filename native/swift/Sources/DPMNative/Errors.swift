// The typed ways the bridge can fail, kept apart from the typed refusals the application makes.
//
// A refusal from the helper (`NativeError`) is the application speaking: it names a stable code and
// says nothing happened, except where a code says otherwise. Everything here is the bridge itself:
// the process, the frames, the clock, the caller. Each transport failure says whether the request
// had been sent, because a request that was sent may have been processed, and a command that was
// processed is committed. Nothing in this library ever resends a request on its own.

import Foundation

/// Whether the request was handed to the helper when the failure happened.
public enum RequestFate: Sendable, Equatable {
    /// It was never written: it cannot have been processed.
    case notSent
    /// It was written, in whole or in part. The helper may have processed it, and a command in it
    /// may have committed. Ask the application, or resend the same command with the same operation
    /// identity on purpose; never assume it was rolled back.
    case mayHaveBeenProcessed
}

/// How the helper process ended.
public struct HelperExit: Sendable, Equatable, CustomStringConvertible {
    public enum Reason: Sendable, Equatable {
        case exited
        case uncaughtSignal
    }

    public let status: Int32
    public let reason: Reason
    /// The last of what the helper wrote to its diagnostic stream, bounded.
    public let stderrTail: String

    public var description: String {
        let how = reason == .exited ? "exited with status \(status)" : "was killed by signal \(status)"
        return stderrTail.isEmpty ? "the helper \(how)" : "the helper \(how): \(stderrTail)"
    }
}

/// How ending a helper went. Success never hides how hard the helper had to be pushed.
public enum ShutdownOutcome: Sendable, Equatable, CustomStringConvertible {
    /// Its input was closed and it exited by itself.
    case exitedOnClose(HelperExit)
    /// It did not exit by itself and was sent SIGTERM.
    case terminated(HelperExit)
    /// It ignored SIGTERM and was sent SIGKILL.
    case killed(HelperExit)
    /// It had already exited before shutdown began.
    case alreadyExited(HelperExit)

    public var description: String {
        switch self {
        case .exitedOnClose(let exit): return "it exited when its input closed: \(exit)"
        case .terminated(let exit): return "it was sent SIGTERM: \(exit)"
        case .killed(let exit): return "it was sent SIGKILL: \(exit)"
        case .alreadyExited(let exit): return "it had already exited: \(exit)"
        }
    }
}

/// What a connection is attached to.
public struct Identity: Sendable, Equatable, CustomStringConvertible {
    public let workspaceId: String
    public let lineageId: String?
    /// `preview`, `live` or `archive`.
    public let source: String

    public var description: String { "\(source) workspace \(workspaceId) lineage \(lineageId ?? "none")" }
}

/// A command the helper committed, named so that it can be reconciled without guessing.
public struct CommittedIdentity: Sendable, Equatable {
    public let operationId: String
    public let resultingRevision: UInt64?
    public let lineageId: String?
}

public enum BridgeError: Error, CustomStringConvertible {
    /// The helper could not be started.
    case launchFailed(String)
    /// The helper started, could not open the workspace, and said why in a frame before exiting.
    case startupRefused(NativeError)
    /// The helper speaks a protocol or an envelope version this client does not.
    case incompatible(String)
    /// The application refused the request. Nothing was written unless the code says otherwise.
    case refused(NativeError)
    /// The helper exited.
    case helperExited(HelperExit, fate: RequestFate)
    /// The connection is closed, or was closed while the request waited.
    case connectionClosed(fate: RequestFate)
    /// The caller cancelled. If the request was sent it was not stopped.
    case cancelled(fate: RequestFate)
    /// No answer came in time. The connection is closed; the request may have been processed.
    case timedOut(fate: RequestFate)
    /// An answer was longer than this client accepts. The connection is closed.
    case frameTooLarge(limit: Int, fate: RequestFate)
    /// A request was longer than the helper accepts, so it was not sent.
    case requestTooLarge(limit: Int)
    /// Too many requests are waiting for the one exchange in flight; this one was not admitted.
    case busy(limit: Int)
    /// The configuration cannot be run: a bound or a timeout that is not a finite positive number.
    case invalidConfiguration(String)
    /// An answer was not valid UTF-8, was truncated, or was not a response. The connection is closed.
    case invalidFrame(String, fate: RequestFate)
    /// An answer did not correlate with the one request in flight. The connection is closed.
    case responseMismatch(expected: String, actual: String, fate: RequestFate)
    /// The helper did not stop within the bounds even after SIGKILL.
    case shutdownFailed(String)
    /// A reconnect reached another source than the one this connection was attached to.
    case sourceIdentityChanged(expected: Identity, found: Identity)
    /// A command's outcome is unknown: it was sent and no answer arrived. It may have committed.
    /// Reconcile with the same request, which carries the same operation identity.
    case commandOutcomeUnknown(operationId: String, cause: String)
    /// A command committed but its answer could not be delivered whole.
    case commandCommitted(CommittedIdentity)

    public var description: String {
        switch self {
        case .launchFailed(let reason): return "could not start the helper: \(reason)"
        case .startupRefused(let error): return "the helper could not open the workspace: \(error)"
        case .incompatible(let reason): return "incompatible helper: \(reason)"
        case .refused(let error): return "refused: \(error)"
        case .helperExited(let exit, let fate): return "\(exit) (\(fate))"
        case .connectionClosed(let fate): return "the connection is closed (\(fate))"
        case .cancelled(let fate): return "cancelled (\(fate))"
        case .timedOut(let fate): return "no answer in time (\(fate))"
        case .frameTooLarge(let limit, let fate): return "an answer exceeded the \(limit)-byte frame bound (\(fate))"
        case .requestTooLarge(let limit): return "the request is over the \(limit)-byte frame bound and was not sent"
        case .busy(let limit): return "\(limit) requests are already waiting; this one was not sent"
        case .invalidConfiguration(let reason): return "invalid configuration: \(reason)"
        case .invalidFrame(let reason, let fate): return "invalid frame: \(reason) (\(fate))"
        case .responseMismatch(let expected, let actual, let fate): return "response \(actual) does not answer request \(expected) (\(fate))"
        case .shutdownFailed(let reason): return "the helper did not stop: \(reason)"
        case .sourceIdentityChanged(let expected, let found): return "reattached to another source: expected \(expected), found \(found)"
        case .commandOutcomeUnknown(let id, let cause): return "outcome of command \(id) is unknown: \(cause)"
        case .commandCommitted(let identity): return "command \(identity.operationId) committed; its answer was not delivered"
        }
    }

    /// The fate of the request this failure ended, when it is a transport failure.
    public var fate: RequestFate? {
        switch self {
        case .helperExited(_, let fate), .connectionClosed(let fate), .cancelled(let fate),
             .timedOut(let fate), .invalidFrame(_, let fate), .frameTooLarge(_, let fate),
             .responseMismatch(_, _, let fate):
            return fate
        case .requestTooLarge, .busy:
            return .notSent
        default:
            return nil
        }
    }
}
