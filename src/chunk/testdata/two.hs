module Numbers where

import Data.List (foldl')

-- First.
one :: Int -> Int
one x =
  let y = x + 1
   in y * 2

-- | Second.
two :: [Int] -> Int
two items =
  foldl' step 0 items
  where
    step total item = total + item
