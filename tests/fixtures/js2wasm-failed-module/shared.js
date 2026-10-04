const token = { marker: 42 };
globalThis.moduleThrownToken = token;
throw token;
