import { type SidecarDescribe } from "../types.js";
interface Props {
    wasmPath: string;
    abiVersion: number;
    describe: SidecarDescribe | null;
}
export declare function Header({ wasmPath, abiVersion, describe }: Props): import("react/jsx-runtime").JSX.Element;
export {};
