// Request/response over the web view's IPC channel. The backend answers by calling
// window.__spannerReply(id, {ok, data | error}).

const pending = new Map();
let seq = 0;

export class Superseded extends Error {}

window.__spannerReply = (id, res) => {
  const p = pending.get(id);
  if (!p) {
    if (!res.ok) console.error("spanner:", res.error);
    return;
  }
  pending.delete(id);
  if (res.ok) p.resolve(res.data);
  else if (res.error === "superseded") p.reject(new Superseded());
  else p.reject(new Error(res.error));
};

export function call(method, params = null) {
  return new Promise((resolve, reject) => {
    seq = (seq % 0x7fffffff) + 1;
    pending.set(seq, { resolve, reject });
    window.ipc.postMessage(JSON.stringify({ id: seq, method, params }));
  });
}
