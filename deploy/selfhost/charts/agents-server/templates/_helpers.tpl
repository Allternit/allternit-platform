{{/* Chart name, truncated to the 63-character label limit. */}}
{{- define "agents-server.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Full name: <release>-<chart>, unless the release name already contains it. */}}
{{- define "agents-server.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "agents-server.selectorLabels" -}}
app.kubernetes.io/name: {{ include "agents-server.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "agents-server.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
{{ include "agents-server.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: allternit-selfhost
{{- end -}}

{{- define "agents-server.image" -}}
{{- printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) -}}
{{- end -}}

{{- define "agents-server.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "agents-server.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/* Name of the Secret that holds the server's secret environment variables. */}}
{{- define "agents-server.secretName" -}}
{{- if .Values.secret.create -}}
{{- printf "%s-env" (include "agents-server.fullname" .) -}}
{{- else -}}
{{- required "secret.existingSecret is required when secret.create is false" .Values.secret.existingSecret -}}
{{- end -}}
{{- end -}}

{{/*
The settings file. Starts from .Values.settings and fixes the parts the chart
owns: the listen addresses (0.0.0.0 inside the pod) and the Prometheus switch.
The Anthropic defaults (credentials.api_key, model_map) are added here, only
for provider anthropic, so they never leak into another provider's settings
through Helm's map merge.
*/}}
{{- define "agents-server.settings" -}}
{{- $s := deepCopy .Values.settings -}}
{{- $inf := default (dict) $s.inference -}}
{{- if eq (toString (default "anthropic" $inf.provider)) "anthropic" -}}
{{- if not $inf.credentials -}}
{{- $_ := set $inf "credentials" (dict "api_key" "env:ANTHROPIC_API_KEY") -}}
{{- end -}}
{{- if not $inf.model_map -}}
{{- $_ := set $inf "model_map" (dict "claude-sonnet-4-5" "claude-sonnet-4-5") -}}
{{- end -}}
{{- end -}}
{{- $_ := set $s "inference" $inf -}}
{{- $server := merge (dict "listen" (printf "0.0.0.0:%d" (int .Values.service.port))) (default (dict) $s.server) -}}
{{- $_ := set $s "server" $server -}}
{{- $prom := dict "enabled" .Values.metrics.enabled "listen" (printf "0.0.0.0:%d" (int .Values.metrics.port)) -}}
{{- $obs := merge (dict "prometheus" $prom) (default (dict) $s.observability) -}}
{{- $_ := set $s "observability" $obs -}}
{{- toYaml $s -}}
{{- end -}}

{{/* Environment shared by the server and the migrate Job. */}}
{{- define "agents-server.env" -}}
- name: ALLTERNIT_SELFHOST_CONFIG
  value: /etc/allternit/settings.yaml
- name: ALLTERNIT_SELF_HOSTED
  value: "1"
{{- range .Values.secret.keys }}
- name: {{ . }}
  valueFrom:
    secretKeyRef:
      name: {{ include "agents-server.secretName" $ }}
      key: {{ . }}
      optional: {{ has . $.Values.secret.optionalKeys }}
{{- end }}
{{- with .Values.otel.endpoint }}
- name: OTEL_EXPORTER_OTLP_ENDPOINT
  value: {{ . | quote }}
{{- end }}
- name: OTEL_SERVICE_NAME
  value: {{ .Values.otel.serviceName | quote }}
{{- with .Values.otel.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- with .Values.extraEnv }}
{{ toYaml . }}
{{- end }}
{{- end -}}

{{/*
A settings duration ("45s", "1500ms", "2m", "1h") as whole seconds, rounded up.
Same pattern as the settings schema: a positive number and one unit.
*/}}
{{- define "agents-server.durationSeconds" -}}
{{- $d := toString . -}}
{{- if not (regexMatch "^[0-9]*[1-9][0-9]*(ms|s|m|h)$" $d) -}}
{{- fail (printf "settings.server.shutdown_grace %q is not a duration like 45s, 1500ms, 2m or 1h" $d) -}}
{{- end -}}
{{- $n := regexFind "^[0-9]+" $d | int64 -}}
{{- $unit := regexFind "(ms|s|m|h)$" $d -}}
{{- if eq $unit "ms" -}}{{ div (add $n 999) 1000 }}
{{- else if eq $unit "s" -}}{{ $n }}
{{- else if eq $unit "m" -}}{{ mul $n 60 }}
{{- else -}}{{ mul $n 3600 }}
{{- end -}}
{{- end -}}

{{/*
terminationGracePeriodSeconds for the server pod: server.shutdown_grace (how
long a stopping replica waits for running turns, default 45s) plus 15 seconds
to fail the turns that are left and close the database pool. An explicit
.Values.terminationGracePeriodSeconds may raise it, never lower it.
*/}}
{{- define "agents-server.terminationGrace" -}}
{{- $grace := include "agents-server.durationSeconds" (dig "server" "shutdown_grace" "45s" (default (dict) .Values.settings)) | int64 -}}
{{- $min := add $grace 15 -}}
{{- with .Values.terminationGracePeriodSeconds -}}
{{- if lt (int64 .) $min -}}
{{- fail (printf "terminationGracePeriodSeconds %d is below settings.server.shutdown_grace + 15s (%d). Raise it or leave it empty." (int64 .) $min) -}}
{{- end -}}
{{- int64 . -}}
{{- else -}}
{{- $min -}}
{{- end -}}
{{- end -}}

{{/*
Fails the render when more than one replica would run on a single-node store.
The server uses SQLite in data_dir (or memory with ephemeral: true) when
settings.database.url is not set; both are per pod. A secret reference
(env:NAME, file:, vault:) always resolves to a Postgres URL; when the chart
creates the Secret itself (secret.create) the value behind env:NAME is checked too.
*/}}
{{- define "agents-server.checkReplicas" -}}
{{- if gt (int .Values.replicaCount) 1 -}}
{{- $url := toString (dig "database" "url" "" (default (dict) .Values.settings)) -}}
{{- if or (eq $url "") (eq $url "<nil>") -}}
{{- fail (printf "replicaCount is %d, but settings.database.url is not set, so each pod would use its own SQLite file (or memory). More than one replica needs PostgreSQL: set settings.database.url (default env:ALLTERNIT_DATABASE_URL, with a postgres:// URL in the Secret), or set replicaCount: 1." (int .Values.replicaCount)) -}}
{{- end -}}
{{- if and .Values.secret.create (hasPrefix "env:" $url) -}}
{{- $v := toString (index (default (dict) .Values.secret.values) (trimPrefix "env:" $url) | default "") -}}
{{- if and $v (not (or (hasPrefix "postgres://" $v) (hasPrefix "postgresql://" $v))) -}}
{{- fail (printf "replicaCount is %d, but secret.values.%s is not a postgres:// URL. More than one replica needs PostgreSQL, or set replicaCount: 1." (int .Values.replicaCount) (trimPrefix "env:" $url)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
