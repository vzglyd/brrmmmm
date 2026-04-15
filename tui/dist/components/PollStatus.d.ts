import { type PollStrategy, type SidecarPhase } from "../types.js";
interface Props {
    phase: SidecarPhase;
    sleepUntilMs: number | null;
    lastSuccessAt: string | null;
    consecutiveFailures: number;
    backoffMs: number | null;
    pollStrategy?: PollStrategy;
    persistenceAuthority?: string;
}
export declare function PollStatus({ phase, sleepUntilMs, lastSuccessAt, consecutiveFailures, backoffMs, pollStrategy, persistenceAuthority, }: Props): import("react/jsx-runtime").JSX.Element;
export {};
