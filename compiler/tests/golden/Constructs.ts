// Every construct the lowering emits code for. Its compiled module is recorded
// in Constructs.module.json: the lowering must keep producing exactly it.
export default class Constructs extends ScriptClass {
    count: int = 3;
    ratio: number = 0.5;
    label: string = "k";
    armed: boolean = true;
    origin: Vec3 = Vec3.new_(1, 2, 3);
    other: Vec3 = Vec3.new_(0, 0, 0);

    begin_play(): void {
        this.count = -4;
        this.ratio = -1.5;
    }

    tick(delta: number): void {
        this.ratio += delta;
        this.ratio -= 0.25;
        this.ratio *= 2;
        this.ratio /= 3;
        this.count++;
        --this.count;
        let before: int = this.count++;
        let step: Vec3 = Vec3.new_(this.origin.x * delta, 0, this.origin.z * delta);
        this.other = this.other.add(step);
        if (this.ratio >= 4.0 && this.armed || !this.armed) {
            this.label = "hot";
        } else if (this.ratio < 1) {
            this.label = "cold";
        } else {
            this.label = this.armed ? "armed" : "idle";
        }
        const same: boolean = this.origin == this.other;
        const differ: boolean = this.origin != this.other;
        if (same) { this.armed = differ; }
    }

    total(limit: int): int {
        let sum: int = 0;
        for (let i = 0; i < limit; i++) {
            if (i % 2 == 0) { continue; }
            if (i > 100) { break; }
            sum += i;
        }
        let n: int = limit;
        while (n > 0) {
            n -= 1;
            if (n == 5) { break; }
        }
        for (;;) { break; }
        { let scoped: int = n; sum += scoped; }
        return sum;
    }

    pick(flag: boolean, a: number, b: number): number {
        if (flag) { return a; }
        return b;
    }

    convert(x: int, y: number): string {
        let f: number = x as number;
        let i: int = y as int;
        return (f + 1) as string + (i as string) + (this.armed as string);
    }

    root(x: number): number { return math.sqrt(x); }

    sameThing(e: Entity): boolean { return e == this.entity; }

    uptime(): number { return this.time; }

    async open(): Promise<void> {
        await wait(1.5);
        this.count += 1;
        await this.settle();
        this.label = "open";
    }

    async settle(): Promise<void> {
        await wait(0.25);
    }

    helper(x: int): int { return this.total(x) + 1; }

    _hidden(): void {}
}
