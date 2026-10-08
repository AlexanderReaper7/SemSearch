(ns demo.numbers
  (:require [clojure.string :as str]))

;; First.
(defn one [x]
  (let [y (inc x)]
    (* y 2)))

;; Second.
(defn two
  "Sums the items."
  [items]
  (reduce (fn [total item]
            (+ total item))
          0
          items))
