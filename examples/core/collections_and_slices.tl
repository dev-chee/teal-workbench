@edition("2026");

// Tier Core：数组、映射、索引、尾部索引、切片、修改和遍历。
// 字符串按 Unicode scalar 索引，len/find 等方法按 UTF-8 字节计数。
task main() {
    let empty = [];
    let numbers = [10, 20, 30, 40,];
    let matrix: int[][] = [[1, 2], [3, 4]];
    let scores: {string: int} = {
        "alice": 10,
        "bob": 20,
    };
    let typed_empty_scores: {string: int} = {};
    let inferred_empty_scores = {};
    inferred_empty_scores.insert("dave", 40);

    let first = numbers[0];
    let last = numbers[-1];
    let middle = numbers[1..3];
    let prefix = numbers[..2];
    let suffix = numbers[2..];
    let full_copy = numbers[..];
    let character = "Teal"[1];
    let substring = "language"[1..5];
    let string_suffix = "hello world"[6..];

    numbers[0] = 11;
    numbers[1] += 2;
    numbers[-1] = 41;
    matrix[1][0] = 9;
    matrix[-1][-1] = 8;
    scores["carol"] = 30;
    scores["alice"] += 1;

    numbers.push(50);
    let total = 0;
    for number in numbers {
        total += number;
    }

    let score_total = 0;
    for name, score in scores {
        print(name, score);
        score_total += score;
    }
    let name_bytes = 0;
    for name in scores {
        name_bytes += name.len();
    }

    print(typeof(empty), first, last, middle, prefix, suffix, full_copy, character, substring, string_suffix);
    print(numbers.len(), matrix[1][0], matrix[-1][-1], scores["carol"], scores["alice"], scores.len(), typed_empty_scores.len(), inferred_empty_scores["dave"], total, score_total, name_bytes);
}
