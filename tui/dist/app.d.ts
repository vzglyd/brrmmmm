interface AppProps {
    wasmPath: string;
    rustBin: string;
    extraArgs: string[];
}
export declare function App({ wasmPath, rustBin, extraArgs }: AppProps): import("react/jsx-runtime").JSX.Element;
export {};
