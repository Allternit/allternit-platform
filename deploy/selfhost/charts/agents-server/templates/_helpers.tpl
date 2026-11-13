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
