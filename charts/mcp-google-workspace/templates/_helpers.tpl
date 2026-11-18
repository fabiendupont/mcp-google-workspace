{{/*
Chart name, fullname and labels.

The chart and the plain manifests in deploy/kubernetes/ describe the same
workload. Labels necessarily differ (Helm adds release-scoped and chart labels,
kustomize adds part-of/managed-by); scripts/chart-drift.py compares everything
that must not drift instead, such as the pod spec.
*/}}
{{- define "mcp-google-workspace.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "mcp-google-workspace.fullname" -}}
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

{{- define "mcp-google-workspace.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "mcp-google-workspace.selectorLabels" -}}
app.kubernetes.io/name: {{ include "mcp-google-workspace.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "mcp-google-workspace.labels" -}}
helm.sh/chart: {{ include "mcp-google-workspace.chart" . }}
{{ include "mcp-google-workspace.selectorLabels" . }}
{{- if .Chart.AppVersion }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
{{- end }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: mcp-google-workspace
{{- end -}}

{{- define "mcp-google-workspace.serviceAccountName" -}}
{{- default (include "mcp-google-workspace.fullname" .) .Values.serviceAccount.name -}}
{{- end -}}

{{/*
Digest-only image reference. `required` produces an error naming the value,
which is much better than rendering a tag and discovering the mistake as an
ImagePullBackOff loop with a moved `:latest` behind it.
*/}}
{{- define "mcp-google-workspace.image" -}}
{{- $digest := required "image.digest is required: this chart deploys by digest, never by tag. Get the released digest with `docker buildx imagetools inspect ghcr.io/fabiendupont/mcp-google-workspace:<version>` and pass --set image.digest=sha256:..." .Values.image.digest -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" $digest) -}}
{{- fail (printf "image.digest must be a sha256:<64 lowercase hex> digest, got %q" $digest) -}}
{{- end -}}
{{- printf "%s@%s" .Values.image.repository $digest -}}
{{- end -}}

{{- define "mcp-google-workspace.tokenSecretName" -}}
{{- default (printf "%s-auth" (include "mcp-google-workspace.fullname" .)) .Values.auth.existingSecret -}}
{{- end -}}

{{- define "mcp-google-workspace.policyConfigMapName" -}}
{{- if .Values.policy.existingConfigMap -}}
{{- .Values.policy.existingConfigMap -}}
{{- else -}}
{{- default (printf "%s-policy" (include "mcp-google-workspace.fullname" .)) .Values.policy.configMapName -}}
{{- end -}}
{{- end -}}

{{/*
Host headers rmcp accepts. The server binds 0.0.0.0, so requests arrive with a
Service DNS name rather than a loopback name and would otherwise get a 403.
*/}}
{{- define "mcp-google-workspace.allowedHosts" -}}
{{- if .Values.config.allowedHosts -}}
{{- .Values.config.allowedHosts -}}
{{- else -}}
{{- $fullname := include "mcp-google-workspace.fullname" . -}}
{{- $namespace := .Release.Namespace -}}
{{- $hosts := list $fullname (printf "%s.%s" $fullname $namespace) (printf "%s.%s.svc" $fullname $namespace) (printf "%s.%s.svc.%s" $fullname $namespace .Values.config.clusterDomain) -}}
{{- join "," (concat $hosts (default (list) .Values.config.extraAllowedHosts)) -}}
{{- end -}}
{{- end -}}

{{/*
The policy document as pretty JSON.

Go's JSON encoder escapes <, > and & as \u003c, \u003e and \u0026 for HTML
safety. The escaping is valid JSON and the server decodes it identically, but a
policy an operator reads back with jsonpath should show `<FOLDER_ID>`, not
`\u003cFOLDER_ID\u003e`, so the escapes are undone for those three characters.
*/}}
{{- define "mcp-google-workspace.policyJson" -}}
{{- $json := "" -}}
{{- if .Values.policy.raw -}}
{{- $json = mustToPrettyJson (mustFromJson .Values.policy.raw) -}}
{{- else -}}
{{- $json = toPrettyJson .Values.policy.document -}}
{{- end -}}
{{- $json | replace "\\u003c" "<" | replace "\\u003e" ">" | replace "\\u0026" "&" -}}
{{- end -}}

{{/*
Reject configurations the server cannot run with, at render time.
*/}}
{{- define "mcp-google-workspace.validate" -}}
{{- if and .Values.auth.existingSecret .Values.auth.token -}}
{{- fail "set either auth.existingSecret or auth.token, not both" -}}
{{- end -}}
{{- if not (or .Values.auth.existingSecret .Values.auth.token) -}}
{{- fail "a bearer token is required: set auth.existingSecret (preferred) or auth.token. The server binds 0.0.0.0 and refuses to start without one, because /mcp and /metrics would otherwise be open to every policy-allowed tool" -}}
{{- end -}}
{{- if not .Values.credentials.existingSecret -}}
{{- fail "credentials.existingSecret is required: create a Secret from your Google service-account JSON (`kubectl create secret generic <name> --from-file=credentials.json=...`) and set credentials.existingSecret" -}}
{{- end -}}
{{- if not (or .Values.policy.existingConfigMap .Values.policy.raw .Values.policy.document) -}}
{{- fail "no policy configured: set policy.existingConfigMap, policy.raw (--set-file) or policy.document. Running without a policy file drops every service scoping rule" -}}
{{- end -}}
{{- end -}}
