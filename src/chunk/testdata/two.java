package demo;

public class Numbers {
    // First.
    public static int one(int x) {
        int y = x + 1;
        return y * 2;
    }

    /** Second. */
    @Deprecated
    public static int two(int[] items) {
        int total = 0;
        for (int item : items) {
            total += item;
        }
        return total;
    }
}
