export function checkAlarmSurface(storage) {
  return ["getAlarm", "setAlarm", "deleteAlarm"].every(
    (method) => typeof storage[method] === "function",
  );
}
