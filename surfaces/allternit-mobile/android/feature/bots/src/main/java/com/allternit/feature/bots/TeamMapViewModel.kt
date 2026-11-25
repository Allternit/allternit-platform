package com.allternit.feature.bots

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.allternit.core.auth.AllternitAuth
import com.allternit.core.auth.AuthState

/** iOS TeamMapStore: loads the map once, keeps the last good map on a failed reload, and reloads after every write. */
class TeamMapViewModel(private val client: TeamMapClient = TeamMapClient) {
    sealed interface Phase {
        data object Loading : Phase
        data object Loaded : Phase
        data class Failed(val error: TeamMapError) : Phase
    }

    var phase by mutableStateOf<Phase>(Phase.Loading)
        private set
    var map by mutableStateOf<TeamMap?>(null)
        private set

    suspend fun load() {
        if (AllternitAuth.state.value !is AuthState.SignedIn) {
            phase = Phase.Failed(TeamMapError.SignedOut)
            return
        }
        if (map == null) phase = Phase.Loading
        when (val r = client.map()) {
            is TeamResult.Ok -> { map = r.value; phase = Phase.Loaded }
            is TeamResult.Failed -> if (map == null) phase = Phase.Failed(r.error)
        }
    }

    /** Each write returns null when it worked, or one plain sentence to show under the control. */
    suspend fun setLevel(grant: TeamGrant, level: String): String? = write { client.setLevel(grant, level) }
    suspend fun setReportsTo(userId: String, managerUserId: String?): String? = write { client.setReportsTo(userId, managerUserId) }
    suspend fun createRule(input: TeamRuleInput): String? = write { client.createRule(input) }
    suspend fun deleteRule(id: String): String? = write { client.deleteRule(id) }
    suspend fun decide(ask: TeamAsk, approve: Boolean): String? = write { client.resolve(ask, approve) }

    private suspend fun write(body: suspend () -> TeamResult<Unit>): String? = when (val r = body()) {
        is TeamResult.Ok -> { load(); null }
        is TeamResult.Failed -> r.error.text
    }
}
