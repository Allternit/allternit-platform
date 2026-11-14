{{- define "sandboxes.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "sandboxes.fullname" -}}
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

{{- define "sandboxes.labels" -}}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" }}
app.kubernetes.io/name: {{ include "sandboxes.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: allternit-selfhost
{{- end -}}

{{/* Selector labels for one environment's workers. Call with (dict "root" $ "env" $env). */}}
{{- define "sandboxes.envSelectorLabels" -}}
app.kubernetes.io/name: {{ include "sandboxes.name" .root }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: worker
allternit.com/environment: {{ .env.name | quote }}
{{- end -}}

{{/* Resource name for one environment: <fullname>-<env name>. */}}
{{- define "sandboxes.envFullname" -}}
{{- printf "%s-%s" (include "sandboxes.fullname" .root) .env.name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "sandboxes.image" -}}
{{- printf "%s:%s" .Values.image.repository (default .Chart.AppVersion .Values.image.tag) -}}
{{- end -}}

{{- define "sandboxes.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "sandboxes.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- default "default" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/*
The worker tools file for one merged environment, as JSON. Checks each entry:
a tool name the model accepts, and a non-empty command list.
*/}}
{{- define "sandboxes.toolsFile" -}}
{{- $tools := dict -}}
{{- range $name, $t := .customTools -}}
{{- if not (regexMatch "^[a-zA-Z0-9_-]{1,64}$" $name) -}}
{{- fail (printf "environments[%s].customTools: tool name %q must be 1-64 letters, digits, _ or -" $.name $name) -}}
{{- end -}}
{{- $cmd := dig "command" list (default (dict) $t) -}}
{{- if or (not (kindIs "slice" $cmd)) (eq (len $cmd) 0) -}}
{{- fail (printf "environments[%s].customTools.%s.command must be a non-empty list (argv)" $.name $name) -}}
{{- end -}}
{{- $entry := dict "command" (list) -}}
{{- range $cmd -}}
{{- $_ := set $entry "command" (append $entry.command (toString .)) -}}
{{- end -}}
{{- with $t.timeoutMs -}}
{{- $_ := set $entry "timeout_ms" (int64 .) -}}
{{- end -}}
{{- $_ := set $tools $name $entry -}}
{{- end -}}
{{- toPrettyJson (dict "tools" $tools) -}}
{{- end -}}
