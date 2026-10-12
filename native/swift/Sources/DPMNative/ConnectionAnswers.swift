// How one answer read from the helper is accepted or refused, before any caller sees it: the frame
// read within the exchange's deadline, checked against the request it answers and against what was
// negotiated, and the failures that leave the byte stream out of step.

import Foundation

extension NativeConnection {
    func readAnswer(_ exchange: Exchange, helper: HelperProcess, protocols: Set<Int>?, api: Int?) -> Result<Response, BridgeError> {
        switch helper.reader.nextLine(deadline: exchange.deadline) {
        case .line(let data):
            // The deadline bounds the whole exchange: an answer that is only now accepted, however
            // it arrived, is a late answer.
            if expired(exchange.deadline) { return .failure(.timedOut(fate: exchange.fate)) }
            if String(data: data, encoding: .utf8) == nil {
                return .failure(.invalidFrame("an answer is not valid UTF-8", fate: exchange.fate))
            }
            let response: Response
            do {
                response = try JSONDecoder().decode(Response.self, from: data)
            } catch {
                return .failure(.invalidFrame("an answer is not a response: \(error)", fate: exchange.fate))
            }
            // The helper answers in order, one per request. A refusal of the frame itself has no
            // identifier to echo, so it answers the request in flight.
            guard response.id == exchange.id || (response.id.isEmpty && !response.ok) else {
                return .failure(.responseMismatch(expected: exchange.id, actual: response.id, fate: exchange.fate))
            }
            if let reason = Self.inconsistency(response, call: exchange.call, protocols: protocols, api: api) {
                return .failure(.invalidFrame(reason, fate: exchange.fate))
            }
            if expired(exchange.deadline) { return .failure(.timedOut(fate: exchange.fate)) }
            return .success(response)
        case .endOfInput:
            let truncated = helper.reader.pending > 0
            if helper.waitForExit(timeout: Self.exitEventBound), let exit = helper.exitInfo {
                return .failure(truncated ? .invalidFrame("the helper exited inside a frame: \(exit)", fate: exchange.fate) : .helperExited(exit, fate: exchange.fate))
            }
            return .failure(truncated ? .invalidFrame("the helper's output ended inside a frame", fate: exchange.fate) : .connectionClosed(fate: exchange.fate))
        case .timedOut:
            return .failure(.timedOut(fate: exchange.fate))
        case .aborted:
            return .failure(.connectionClosed(fate: exchange.fate))
        case .tooLarge:
            return .failure(.frameTooLarge(limit: configuration.maxResponseBytes, fate: exchange.fate))
        }
    }

    /// Whether an answer contradicts what was negotiated: another protocol than the one agreed, or
    /// an envelope of another api version than the helper announced.
    /// Before a protocol is chosen (`protocols` is nil) any version may answer, since the helper
    /// answers an offer it cannot meet in its own.
    ///
    /// Everything that makes an answer well formed for the call it answers is decided here, before
    /// the answer is accepted, so a bad one invalidates the connection before any caller sees it:
    /// exactly one of result and error, agreeing with `ok`; a result of the kind the call asks for;
    /// and the negotiated api version on every envelope and on every refusal.
    private static func inconsistency(_ response: Response, call: Call, protocols: Set<Int>?, api: Int?) -> String? {
        if let protocols = protocols, !protocols.contains(response.protocolVersion) {
            return "an answer is in protocol \(response.protocolVersion), not the negotiated \(protocols.sorted())"
        }
        if response.ok != (response.error == nil) || response.ok != (response.result != nil) {
            return "an answer's ok flag (\(response.ok)) disagrees with its content (result \(response.result != nil ? "present" : "absent"), error \(response.error != nil ? "present" : "absent"))"
        }
        if let kind = response.result, !kind.answers(call) {
            return "an answer of kind \(kind.kind) does not answer a \(call.kind) call"
        }
        guard let api = api else { return nil }
        switch response.result {
        case .view(let view)? where view.envelope.apiVersion != api:
            return "a view is in api version \(view.envelope.apiVersion), not the negotiated \(api)"
        case .committed(let committed)? where committed.envelope.apiVersion != api:
            return "a commit is in api version \(committed.envelope.apiVersion), not the negotiated \(api)"
        default:
            break
        }
        if let error = response.error, error.apiVersion != api {
            return "a refusal is in api version \(error.apiVersion), not the negotiated \(api)"
        }
        return nil
    }

    /// Whether a failure leaves the byte stream out of step or the helper untrustworthy.
    func desynchronizes(_ error: BridgeError) -> Bool {
        switch error {
        case .helperExited, .timedOut, .frameTooLarge, .invalidFrame, .responseMismatch: return true
        case .connectionClosed(let fate): return fate == .mayHaveBeenProcessed
        default: return false
        }
    }
}
