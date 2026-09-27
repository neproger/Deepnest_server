type NestingConfig = {
  units?: "mm" | "inch";
  scale?: number;
  spacing?: number;
  curveTolerance?: number;
  /** number of evenly spaced orientations (4 → 0/90/180/270) or an angle list */
  rotations?: number | number[];
  /** samples per item placement */
  budget?: number;
  strategy?: "sampling" | "nfp";
  separationEffort?: "full" | "fast" | "max" | "off";
  columnWeight?: number;
};

type NestingOptions = NestingConfig & {
  bin: { width: number; height: number } | string;
  timeout?: number;
  progressCallback?: (data: {
    phase: string;
    index: number;
    progress: number;
    threads: number;
  }) => any;
};

type SheetPlacement = {
  filename: string;
  id: number;
  rotation: number;
  source: number;
  x: number;
  y: number;
};

type NestingResult = {
  area: number;
  fitness: number;
  index: number;
  mergedLength: number;
  selected: boolean;
  placements: {
    sheet: number;
    sheetid: number;
    sheetplacements: SheetPlacement[];
  }[];
};

/**
 * @returns disposer
 */
export function nest(
  svgInput: (
    | string
    | {
        file: string;
        svg: string;
      }
  )[],
  callback: (data: {
    result: SheetPlacement[];
    data: NestingResult;
    elements: SVGElement[];
    status: {
      better: boolean;
      complete: boolean;
      placed: number;
      total: number;
    };
    /** Optional: no engine currently renders results server-side. */
    svg?: () => string;
    abort: () => Promise<void>;
  }) => any,
  options: NestingOptions
): Promise<() => Promise<void>>;
