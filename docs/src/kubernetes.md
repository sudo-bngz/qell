# Kubernetes

`Ctrl/⌘+Shift+K` — or the helm in the activity bar — opens a k9s-style
view of your cluster where the editor is: pods, deployments, services,
nodes, events and anything else the cluster serves, in a list you drive
from the keyboard.

It reads everything through **your own `kubectl`**, so your kubeconfig,
contexts and credential plugins (`aws eks get-token` and the like) are used
as they are. Qell holds no credentials and installs nothing. The list
refreshes every two seconds **only while the view has the keyboard**, and
stops the moment you click elsewhere.

## Moving around

| | |
|---|---|
| `j` / `k` | move |
| `Enter` | go in: a namespace's pods, a workload's pods, a pod's containers |
| `Esc` | come back |
| `/` | filter (suggests the names in front of you) |
| `:` | switch resource: `:po`, `:deploy`, `:svc`, `:ns`, `:ctx`, CRDs too — `Tab` completes |
| `0`–`9` | namespaces: `0` is all of them |
| `r` | refresh |
| `?` | the keys |

Rows are coloured by what the cluster says: a `CrashLoopBackOff` pod or a
`NotReady` node in red, a rollout catching up in yellow.

## Acting

| | |
|---|---|
| `d` / `y` | describe / YAML, in the view (`Esc` back) |
| `e` | open the manifest in a tab — **saving it applies it** to the cluster |
| `s` | a shell in the pod (bash where there is one) |
| `l` / `Shift+L` | follow the logs / the previous container's |
| `f` | forward a port, the object's own port filled in |
| `Ctrl+D` | delete, after a dialog that says what and how (propagation, force) |

Shells, logs and port-forwards run `kubectl` in terminal tabs below the
list, named for what they do; `Ctrl+C` or closing the tab stops them.

## Usage

With metrics-server in the cluster, pods carry `CPU`, `MEM` and their
percentage of requests and limits, nodes theirs, and the header draws the
last minute of cluster CPU and memory. The header also shows the context,
cluster, user and versions, k9s-style.

The cluster view takes the window: the file tree goes with the editor and
comes back with it. Anything that asks for a file — the Explorer,
`Ctrl/⌘+P`, `e` on a manifest — brings the editor back, and
`Ctrl/⌘+Shift+K` returns to the cluster where you left it.
