using System;

namespace Demo;

public static class Numbers
{
    // First.
    public static int One(int x)
    {
        var y = x + 1;
        return y * 2;
    }

    /// <summary>Second.</summary>
    [Obsolete]
    public static int Two(int[] items)
    {
        var total = 0;
        foreach (var item in items)
        {
            total += item;
        }
        return total;
    }
}
