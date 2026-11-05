import { runActuatorFlightSelfCheck } from './actuatorFlight.selfcheck'
import { runBackendVersionSelfCheck } from './backendVersion.selfcheck'
import { runConnectionResilienceSelfCheck } from './connectionResilience.selfcheck'
import { runFirmwareVersionSelfCheck } from './firmwareVersion.selfcheck'
import { runHealthDialogSelfCheck } from './healthDialog.selfcheck'
import { runPendingFieldsSelfCheck } from './pendingFields.selfcheck'

runPendingFieldsSelfCheck()
runActuatorFlightSelfCheck()
runBackendVersionSelfCheck()
runConnectionResilienceSelfCheck()
runHealthDialogSelfCheck()
runFirmwareVersionSelfCheck()
