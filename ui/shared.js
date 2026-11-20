// State fetched once and shared by every page.

import { call } from "./ipc.js";

export const overview = call("overview");
