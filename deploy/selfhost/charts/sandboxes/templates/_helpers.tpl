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
