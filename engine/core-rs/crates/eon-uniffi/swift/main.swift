import Foundation

func smoke() throws {
    let mobile = try selectModelProfile(memoryGb: 4)
    precondition(mobile == "p0_mobile_ultralight")
    do {
        _ = try selectModelProfile(memoryGb: .nan)
        fatalError("accepted NaN")
    } catch BridgeError.InvalidInput(let reason) {
        precondition(reason.contains("finite"))
    }
    precondition(endpointSilenceMs(completeness: .complete) == 300)
    precondition(endpointSilenceMs(completeness: .ambiguous) == 800)
    precondition(endpointSilenceMs(completeness: .incomplete) == 1150)
    let id = try generateUlid(prefix: "task")
    let next = try generateUlid(prefix: "task")
    precondition(id.hasPrefix("task_") && id.count == 31 && id < next)
    let before = getMonotonicTimeNanos()
    precondition(getMonotonicTimeNanos() >= before)
    let probe = try injectAudioPcmForTesting(samples: Array(repeating: Int16.min, count: 512))
    precondition(probe.sampleCount == 512 && probe.outputPeak == 0.95 && probe.appliedGain == 0.95)
    do {
        _ = try injectAudioPcmForTesting(samples: [])
        fatalError("accepted an empty test frame")
    } catch BridgeError.InvalidInput(_) {}
    let result = try executeTaskSync(taskId: id, capability: "diagnostics", operation: "select_model_profile",
                                     parametersJson: #"{"memory_gb":12}"#)
    precondition(result.taskId == id && result.status == "completed" && result.errorCode == nil)
    precondition(result.outcomeJson == #"{"profile":"p2_workstation_16g"}"#)
    do {
        _ = try executeTaskSync(taskId: id, capability: "shell", operation: "run", parametersJson: "{}")
        fatalError("accepted an unknown capability")
    } catch BridgeError.UnsupportedTask(_) {}
}

try smoke()
print("eon-uniffi Swift smoke: all six generated functions OK")
