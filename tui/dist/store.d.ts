import { type BrrEvent, type TuiState } from "./types.js";
export declare function initialState(wasmPath: string): TuiState;
export declare function reducer(state: TuiState, event: BrrEvent): TuiState;
