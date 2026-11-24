// Capture the document fence and strict serializer before author scripts can replace builtins.
(() => {
  const DOCUMENT_WORDS = 4, ERROR_LIMIT = 1000;
  const random = crypto.getRandomValues.bind(crypto), stringify = JSON.stringify, keys = Object.keys;
  const nativeString = String, clip = Function.call.bind(String.prototype.slice);
  const finite = Number.isFinite, isArray = Array.isArray, define = Object.defineProperty;
  const NativeNode = Node, Values = Uint32Array;
  const isNode = value => value instanceof NativeNode;
  const next = () => { const words = random(new Values(DOCUMENT_WORDS)); return `${words[0]}-${words[1]}-${words[2]}-${words[3]}`; };
  let document = next();
  addEventListener('pagehide', event => { if (event.isTrusted) document = next(); }, {capture:true});
  define(globalThis, '__keelDebugMainDocument', {get:() => document, configurable:false});
  define(globalThis, '__keelDebugFailure', {configurable:false,writable:false,value:error => stringify({ok:false,error:clip(nativeString(error),0,ERROR_LIMIT)})});
  define(globalThis, '__keelDebugSerialize', {configurable:false, writable:false, value:value => {
    const seen = new Set();
    const visit = item => {
      if (item === null || typeof item === 'string' || typeof item === 'boolean') return;
      if (typeof item === 'number' && finite(item)) return;
      if (typeof item !== 'object' || isNode(item) || typeof item.then === 'function') throw 'Result must be JSON; promises, DOM nodes and non-JSON values are unsupported.';
      if (seen.has(item)) throw 'Result contains a cycle.';
      seen.add(item);
      for (const key of keys(item)) visit(item[key]);
      if (isArray(item)) for (let i=0;i<item.length;i++) if (!(i in item)) throw 'Sparse arrays are not JSON.';
      seen.delete(item);
    };
    visit(value);
    return stringify(value);
  }});
})();
