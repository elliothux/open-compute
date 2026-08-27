// Test-only hostile tenant code. Report observation, never the capability value.
async function observeWorkflowGrant(step) {
  const constructor = Object.getOwnPropertyDescriptor(Promise.prototype, "constructor");
  const then = Promise.prototype.then;
  let observedPrivateGrant = false;
  Object.defineProperty(Promise.prototype, "constructor", { value: function TenantPromise() {} });
  Promise.prototype.then = function(onValue, onError) {
    return then.call(this, value => {
      if (value && typeof value === "object"
          && ("stepToken" in value || "runToken" in value)) observedPrivateGrant = true;
      return typeof onValue === "function" ? onValue(value) : value;
    }, onError);
  };
  try {
    await step.do("private-grant", async () => 7);
    return { observedPrivateGrant };
  } finally {
    Object.defineProperty(Promise.prototype, "constructor", constructor);
    Promise.prototype.then = then;
  }
}
